<script setup lang="ts">
    import { computed, onMounted, onUnmounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import type { Update } from '@tauri-apps/plugin-updater';
    import { tauri, type UploadStateSnapshot, type PendingUpload } from '../lib/tauri';
    import { checkForUpdate, runUpdate, type UpdateState } from '../lib/updater';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const config = useConfigStore();

    const queue = ref<UploadStateSnapshot>({
        items: [],
        processed_count: 0,
        done_count: 0,
        duplicate_count: 0,
        error_count: 0,
    });
    const toggling = ref(false);
    const toggleError = ref<string | null>(null);
    const paused = ref(false);

    // CPU throttle - live mutable target percentage the worker is using
    // *right now*. Separate from config.cpu_throttle_pct which is the
    // user's saved preference. The Speed-up button cycles through
    // progressively faster tiers and then wraps back to the saved
    // value, rather than a binary toggle - lets the user say "a bit
    // faster" without committing to "absolutely no limit".
    const currentThrottlePct = ref(15);

    const refreshThrottle = async () => {
        try {
            currentThrottlePct.value = await tauri.getCpuThrottlePct();
        } catch { /* watcher not running */ }
    };

    // Rate-limit countdown. Backend stores a unix-epoch-ms timestamp at
    // which the active 429 backoff ends; we poll once a second while
    // the watcher is running and render a banner when > now. The 1s
    // poll is cheap (single AtomicU64 load) and makes the countdown
    // visibly tick without needing event-driven plumbing.
    const rateLimitResumeAtMs = ref(0);
    const nowMs = ref(Date.now());
    const rateLimitSecondsLeft = computed(() => {
        const delta = rateLimitResumeAtMs.value - nowMs.value;
        return delta > 0 ? Math.ceil(delta / 1000) : 0;
    });
    const isRateLimited = computed(() => rateLimitSecondsLeft.value > 0);
    let rateLimitPollTimer: number | undefined;
    let nowTickTimer: number | undefined;

    const pollRateLimit = async () => {
        try {
            rateLimitResumeAtMs.value = await tauri.getRateLimitResumeAtMs();
        } catch { /* watcher not running, leave at 0 */ }
    };

    // Cycle order: user's saved preference first, then any strictly-
    // faster tier (50% Fast, then 0% No-limit). 0 is treated as the
    // fastest because the throttle code interprets 0 as "no idle wait".
    // If the user's saved is already No-limit there's nothing to bump
    // to and the button hides; if it's Fast (50%), the cycle is just
    // [50, 0]; default Background (15%) gives a full [15, 50, 0].
    const speedCycle = computed<number[]>(() => {
        const saved = config.config.cpu_throttle_pct ?? 15;
        const cycle = [saved];
        if (saved > 0 && saved < 50) cycle.push(50);
        if (saved !== 0) cycle.push(0);
        return cycle;
    });

    const showSpeedButton = computed(() => speedCycle.value.length > 1);

    const nextSpeedTier = computed<number>(() => {
        const cycle = speedCycle.value;
        const idx = cycle.findIndex(t => t === currentThrottlePct.value);
        // If current pct doesn't match any tier (unexpected; setter
        // only ever lands on a tier value), treat as if at saved so
        // the next click takes the user one step forward.
        const base = idx >= 0 ? idx : 0;
        return cycle[(base + 1) % cycle.length];
    });

    /** Is the very next click a wrap back to the saved preference?
     *  Used to flip the button to a "slow down" look. */
    const willWrapToSaved = computed(() => {
        const saved = config.config.cpu_throttle_pct ?? 15;
        return nextSpeedTier.value === saved && currentThrottlePct.value !== saved;
    });

    const speedButtonText = computed(() => {
        const next = nextSpeedTier.value;
        if (willWrapToSaved.value) {
            return `Slow down (${next}%)`;
        }
        if (next === 0) return 'Faster (no limit)';
        if (next === 50) return 'Faster (50%)';
        return `Faster (${next}%)`;
    });

    const speedButtonEmoji = computed(() => willWrapToSaved.value ? '🐌' : '🚀');

    const speedButtonTooltip = computed(() => {
        const saved = config.config.cpu_throttle_pct ?? 15;
        const curLabel = currentThrottlePct.value === 0 ? 'no limit' : `${currentThrottlePct.value}% CPU`;
        return `Currently ${curLabel}. Click cycles to the next tier; the cycle wraps back to your saved ${saved}% preference.`;
    });

    const cycleSpeed = async () => {
        const target = nextSpeedTier.value;
        try {
            await tauri.setCpuThrottlePctRuntime(target);
            currentThrottlePct.value = target;
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Speed change failed';
        }
    };

    // (Play button moved into the app-wide top nav in App.vue so it's
    // available from any tab, not just Dashboard.)

    // Per-row expand state. Keyed by path so the same row stays open
    // through queue updates (sort order doesn't shuffle anything; new
    // items appear at the top).
    const expanded = ref<Set<string>>(new Set());
    const toggleExpand = (path: string) => {
        if (expanded.value.has(path)) expanded.value.delete(path);
        else expanded.value.add(path);
        // Force Vue reactivity on Set mutation.
        expanded.value = new Set(expanded.value);
    };

    // Status filter for the queue list. Purely client-side - the
    // backend always sends the full snapshot and the user picks a
    // slice. "All" is the default so existing behaviour is unchanged
    // for anyone who doesn't notice the tabs.
    type QueueFilter = 'all' | 'in_progress' | 'done' | 'duplicate' | 'error';
    const queueFilter = ref<QueueFilter>('all');
    const filteredQueueItems = computed(() => {
        switch (queueFilter.value) {
            case 'in_progress':
                return queue.value.items.filter((i) =>
                    i.status === 'pending' || i.status === 'hashing' || i.status === 'uploading',
                );
            case 'done':       return queue.value.items.filter((i) => i.status === 'done');
            case 'duplicate':  return queue.value.items.filter((i) => i.status === 'duplicate');
            case 'error':      return queue.value.items.filter((i) => i.status === 'error');
            default:           return queue.value.items;
        }
    });

    // defrag:// pending modal lives in App.vue so it floats above
    // every tab. Dashboard no longer owns that state.

    // Auto-update banner. Quiet on success (just shows "up to date" for
    // a beat); persistent on "Update available" until the user installs.
    //
    // We can't store the Update object inside `ref` - its private field
    // doesn't survive Vue's reactive proxy and breaks the TS type. Keep
    // the class instance in a plain `let` and have the ref carry only
    // serializable data needed for the template.
    const updateState = ref<UpdateState>({ kind: 'idle' });
    let pendingUpdate: Update | null = null;

    let unlisten: UnlistenFn | null = null;
    let updateCheckTimer: number | undefined;

    // Re-check for updates every 6h while the launcher is alive. The
    // interval keeps firing even when the main window is hidden in the
    // tray - JS lifecycle is bound to the process, not the OS window -
    // so the "set and forget" user who installed once and stays in
    // tray for weeks still gets caught up on security patches.
    const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

    const checkUpdate = async () => {
        // Auto-update is always on (the Settings toggle was retired -
        // security fixes have to reach every install without depending
        // on a user remembering to flip a switch). We still read the
        // config flag for forward compatibility in case an expert-mode
        // escape hatch reappears, but the default is true and the UI
        // exposes no way to flip it off.
        if (!config.config.auto_update_enabled) return;
        try {
            const update = await checkForUpdate();
            if (update) {
                pendingUpdate = update;
                updateState.value = { kind: 'available', version: update.version };
            }
        } catch {
            // Network blip; the next interval will retry.
        }
    };

    const refreshPaused = async () => {
        try {
            paused.value = await tauri.isAutoUploadPaused();
        } catch {
            paused.value = false;
        }
    };

    onMounted(async () => {
        queue.value = await tauri.getUploadState();
        await refreshPaused();
        await refreshThrottle();
        await pollRateLimit();

        // Poll the rate-limit timestamp once a second; tick the local
        // "now" every 250ms so the countdown feels live without an
        // extra Tauri round-trip per frame.
        rateLimitPollTimer = window.setInterval(pollRateLimit, 1000);
        nowTickTimer = window.setInterval(() => { nowMs.value = Date.now(); }, 250);

        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            queue.value = ev.payload;
        });

        if (config.config.auto_update_enabled) {
            try {
                updateState.value = { kind: 'checking' };
                const update = await checkForUpdate();
                if (update) {
                    pendingUpdate = update;
                    updateState.value = { kind: 'available', version: update.version };
                } else {
                    updateState.value = { kind: 'idle' };
                }
            } catch {
                updateState.value = { kind: 'idle' };
            }
            updateCheckTimer = window.setInterval(checkUpdate, UPDATE_CHECK_INTERVAL_MS);
        }
    });

    onUnmounted(() => {
        if (unlisten) unlisten();
        if (updateCheckTimer !== undefined) window.clearInterval(updateCheckTimer);
        if (rateLimitPollTimer !== undefined) window.clearInterval(rateLimitPollTimer);
        if (nowTickTimer !== undefined) window.clearInterval(nowTickTimer);
    });

    const installUpdate = async () => {
        if (!pendingUpdate) return;
        await runUpdate(pendingUpdate, (s) => { updateState.value = s; });
    };

    const toggle = async () => {
        toggleError.value = null;
        toggling.value = true;
        try {
            if (config.autoUploadRunning) {
                await tauri.stopAutoUpload();
            } else {
                await tauri.startAutoUpload();
            }
            await config.refresh();
            await refreshPaused();
            await refreshThrottle();
        } catch (e: any) {
            toggleError.value = e.toString();
        } finally {
            toggling.value = false;
        }
    };

    const togglePause = async () => {
        try {
            if (paused.value) await tauri.resumeAutoUpload();
            else await tauri.pauseAutoUpload();
            await refreshPaused();
        } catch (e: any) {
            toggleError.value = e.toString();
        }
    };

    const statusLabel = (item: PendingUpload) => {
        switch (item.status) {
            case 'pending': return 'Waiting';
            case 'hashing': return 'Hashing';
            case 'uploading': return 'Uploading';
            case 'done': return 'Uploaded';
            case 'duplicate': return 'Already backed up';
            case 'error': return 'Error';
        }
    };

    const statusColor = (item: PendingUpload) => {
        switch (item.status) {
            case 'done': return 'text-emerald-400';
            case 'duplicate': return 'text-cyan-400';
            case 'error': return 'text-red-400';
            case 'uploading':
            case 'hashing': return 'text-brand-400';
            default: return 'text-neutral-500';
        }
    };

    /** "2.3 MB", "412 KB", "-" for null/zero. */
    const formatBytes = (n: number | null) => {
        if (n == null || n <= 0) return '-';
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
        if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
        return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
    };

    /** Bytes-per-second → human "12.4 MB/s" etc. */
    const formatRate = (bps: number | null) => {
        if (bps == null || bps <= 0) return '-';
        if (bps < 1024) return `${bps} B/s`;
        if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
        return `${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
    };

    const duplicateExplain = (item: PendingUpload) => {
        if (item.status !== 'duplicate') return null;
        if (item.duplicate_reason === 'cache') return 'Skipped: matched local cache (we already uploaded this file before).';
        if (item.duplicate_reason === 'server') return 'Skipped: defrag.racing already has this demo (matched by MD5).';
        return 'Skipped as duplicate.';
    };

    // Open a defrag.racing URL in the system browser via the opener
    // plugin so it doesn't try to render inside our webview. Wrapper
    // shared by the map-page link and the demo-download link below.
    const openWebUrl = async (url: string) => {
        try {
            const { openUrl } = await import('@tauri-apps/plugin-opener');
            await openUrl(url);
        } catch {
            // best effort
        }
    };

    // Defrag demo filename convention: `<mapname>[<physics>]<time>(<player>).dm_*`.
    // Map name is everything before the first `[`. Returns null when the
    // filename doesn't match the convention - keeps the link hidden
    // rather than building a /maps/garbage URL that would 404.
    const mapNameFromFilename = (filename: string): string | null => {
        const idx = filename.indexOf('[');
        if (idx <= 0) return null;
        const name = filename.slice(0, idx).trim();
        return name.length > 0 ? name : null;
    };

    const openMapPage = (filename: string) => {
        const map = mapNameFromFilename(filename);
        if (map) openWebUrl(`https://defrag.racing/maps/${encodeURIComponent(map)}`);
    };

    const openDemoDownload = (id: number) => {
        openWebUrl(`https://defrag.racing/demos/${id}/download`);
    };

    const queueSummary = computed(() => {
        const counts = { uploading: 0, hashing: 0, done: 0, duplicate: 0, error: 0, pending: 0 };
        for (const it of queue.value.items) counts[it.status as keyof typeof counts]++;
        return counts;
    });

    // -- Retry + context menu ------------------------------------------
    const retrying = ref<Set<string>>(new Set());
    const retryUpload = async (item: PendingUpload) => {
        if (retrying.value.has(item.path)) return;
        retrying.value.add(item.path);
        retrying.value = new Set(retrying.value);
        try {
            await tauri.retryUpload(item.path);
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Retry failed';
        } finally {
            retrying.value.delete(item.path);
            retrying.value = new Set(retrying.value);
        }
    };

    type CtxMenu = { x: number; y: number; item: PendingUpload };
    const ctxMenu = ref<CtxMenu | null>(null);
    const openContextMenu = (e: MouseEvent, item: PendingUpload) => {
        e.preventDefault();
        ctxMenu.value = { x: e.clientX, y: e.clientY, item };
    };
    const closeContextMenu = () => { ctxMenu.value = null; };

    const filenameStem = (filename: string): string => {
        const idx = filename.lastIndexOf('.');
        return idx > 0 ? filename.slice(0, idx) : filename;
    };

    const copyToClipboard = async (text: string) => {
        try { await navigator.clipboard.writeText(text); } catch {
            const ta = document.createElement('textarea');
            ta.value = text;
            ta.style.position = 'fixed';
            ta.style.opacity = '0';
            document.body.appendChild(ta);
            ta.select();
            try { document.execCommand('copy'); } catch { /* ignore */ }
            document.body.removeChild(ta);
        }
    };

    const ctxOpenInExplorer = async () => {
        if (!ctxMenu.value) return;
        const p = ctxMenu.value.item.path;
        closeContextMenu();
        try {
            const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
            await revealItemInDir(p);
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Failed to open in explorer';
        }
    };

    const ctxCopyPath = async () => {
        if (!ctxMenu.value) return;
        await copyToClipboard(ctxMenu.value.item.path);
        closeContextMenu();
    };

    const ctxCopyDemoCmd = async () => {
        if (!ctxMenu.value) return;
        await copyToClipboard(`/demo ${filenameStem(ctxMenu.value.item.filename)}`);
        closeContextMenu();
    };

    const ctxDeleteDemo = async () => {
        if (!ctxMenu.value) return;
        const it = ctxMenu.value.item;
        closeContextMenu();
        if (!window.confirm(`Permanently delete ${it.filename}?\n\nThe file on disk will be removed. Anything already on defrag.racing stays.`)) return;
        try { await tauri.deleteDemo(it.path); } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Delete failed';
        }
    };
</script>

<template>
    <div class="flex-1 flex flex-col">
        <!-- top bar -->
        <header class="px-5 py-3 border-b border-white/10 flex items-start justify-between gap-3">
            <div class="flex items-start gap-2 min-w-0">
                <div
                    class="w-2 h-2 rounded-full mt-1.5 flex-shrink-0"
                    :class="!config.autoUploadRunning ? 'bg-neutral-600' : (paused ? 'bg-amber-400' : 'bg-emerald-400')"
                ></div>
                <div class="text-sm min-w-0">
                    <div>
                        <span class="font-semibold">Auto-upload</span>
                        <span class="text-neutral-500 ml-1">
                            {{ !config.autoUploadRunning ? 'off' : (paused ? 'paused' : 'running') }}
                        </span>
                    </div>
                    <div class="text-xs text-neutral-500 mt-0.5 leading-snug">
                        <template v-if="config.autoUploadRunning && !paused">
                            Watching
                            <code class="bg-black/40 px-1 rounded">{{ config.config.demos_path || '(folder not set)' }}</code>
                            live - new <code class="bg-black/40 px-1 rounded">.dm_*</code> files
                            appear within ~30s of recording stop. Every 30 min the
                            folder is re-scanned as a safety net in case the OS
                            drops an event. The scan just lists filenames; hashing
                            is paced by your CPU throttle so it doesn't lag the disk.
                        </template>
                        <template v-else-if="config.autoUploadRunning && paused">
                            Watcher is still picking up new demos, but uploads are paused. Click Resume to drain the queue.
                        </template>
                        <template v-else>
                            Click <strong class="text-brand-400">Start</strong> to watch
                            <code class="bg-black/40 px-1 rounded">{{ config.config.demos_path || 'your demos folder' }}</code>
                            and back up new demos to defrag.racing automatically.
                        </template>
                    </div>
                </div>
            </div>
            <div class="flex items-center gap-2 flex-shrink-0">
                <!-- Speed-up: cycles through progressively faster tiers
                     and wraps back to the user's saved preference.
                     Default cycle is 15% (saved) → 50% (Fast) → 0%
                     (No limit) → 15%. Color flips amber when above the
                     saved tier so the user can see at a glance they're
                     in temporary-fast mode. -->
                <button
                    v-if="config.autoUploadRunning && showSpeedButton"
                    class="px-3 py-1.5 rounded text-sm font-semibold flex items-center gap-1.5"
                    :class="willWrapToSaved
                        ? 'bg-amber-500/20 hover:bg-amber-500/30 text-amber-300'
                        : 'bg-white/5 hover:bg-white/10 text-neutral-200'"
                    :title="speedButtonTooltip"
                    @click="cycleSpeed"
                >
                    <span>{{ speedButtonEmoji }}</span>
                    <span>{{ speedButtonText }}</span>
                </button>
                <button
                    v-if="config.autoUploadRunning"
                    class="px-3 py-1.5 rounded text-sm font-semibold bg-white/5 hover:bg-white/10 text-neutral-200"
                    @click="togglePause"
                >{{ paused ? 'Resume' : 'Pause' }}</button>
                <button
                    class="px-3 py-1.5 rounded text-sm font-semibold"
                    :class="config.autoUploadRunning
                        ? 'bg-white/5 hover:bg-white/10 text-neutral-200'
                        : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-400'"
                    :disabled="toggling"
                    @click="toggle"
                >
                    {{ config.autoUploadRunning ? 'Stop' : 'Start' }}
                </button>
            </div>
        </header>

        <p v-if="toggleError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ toggleError }}
        </p>

        <!-- Rate-limit countdown. Renders whenever the API client is
             currently sleeping on a 429 Retry-After. The launcher
             auto-resumes when the timer hits zero; banner is just
             informational so the user knows nothing is broken. -->
        <div
            v-if="isRateLimited"
            class="px-5 py-2 border-b border-amber-500/20 bg-amber-500/10 text-xs text-amber-200 flex items-center gap-2"
        >
            <span class="text-amber-400">⏳</span>
            <span>
                Rate-limited by defrag.racing - resuming in
                <strong>{{ rateLimitSecondsLeft }}s</strong>.
                The launcher will retry automatically.
            </span>
        </div>

        <!-- No-token banner. Surfaces explicitly which features the user
             is missing so they don't sit on an empty dashboard wondering
             why nothing happens. Direct button to Settings -> token form
             rather than making them hunt for it. -->
        <div
            v-if="config.loaded && !config.hasToken"
            class="px-5 py-3 border-b border-amber-500/30 bg-amber-500/10 text-xs text-amber-100"
        >
            <div class="font-semibold text-amber-200 mb-1">
                No token saved - most launcher features are disabled
            </div>
            <ul class="space-y-0.5 pl-1 mb-2">
                <li>• Auto-backup of recorded demos</li>
                <li>• Server browser with your PB and rank per map</li>
                <li>• Record + system notifications from your account</li>
            </ul>
            <div class="flex items-center justify-between gap-2">
                <span class="text-amber-100/80">
                    Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links work without one.
                </span>
                <button
                    class="px-3 py-1 rounded bg-amber-500/30 hover:bg-amber-500/40 text-amber-100 font-semibold flex-shrink-0"
                    @click="router.push({ name: 'settings' })"
                >
                    Add token →
                </button>
            </div>
        </div>

        <div
            v-if="updateState.kind === 'available'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300 flex items-center gap-3"
        >
            <span>Update <strong>v{{ updateState.version }}</strong> is available.</span>
            <button class="ml-auto px-2 py-0.5 rounded bg-brand-500/20 hover:bg-brand-500/30 font-semibold" @click="installUpdate">
                Install &amp; restart
            </button>
        </div>
        <div
            v-else-if="updateState.kind === 'downloading'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300"
        >
            Downloading update… {{ updateState.percent }}%
        </div>
        <div
            v-else-if="updateState.kind === 'installing'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300"
        >
            Installing… the launcher will restart in a moment.
        </div>
        <div
            v-else-if="updateState.kind === 'error'"
            class="px-5 py-2 border-b border-red-500/20 bg-red-500/10 text-xs text-red-300"
        >
            Update failed: {{ updateState.message }}
        </div>

        <!-- Queue summary strip. The "uploaded / backed up / errors"
             numbers come from the UNBOUNDED session counters on the
             backend, not from counting queue.items by status - the
             queue is capped at QUEUE_CAP rows for DOM perf, and the
             queue-derived numbers would clamp at that ceiling and make
             a 5000+ rescan look frozen. "hashing / uploading" stay
             queue-derived because they represent current in-flight
             work, which by definition is in the visible queue. -->
        <div
            v-if="queue.items.length || queue.processed_count"
            class="px-5 py-2 border-b border-white/[0.04] text-xs text-neutral-400 flex items-center gap-3 flex-wrap"
        >
            <span v-if="queue.processed_count" class="text-neutral-200 font-semibold">
                {{ queue.processed_count }} processed this session
            </span>
            <span v-if="queue.items.length" class="text-neutral-500">
                ({{ queue.items.length }} shown)
            </span>
            <span v-if="queueSummary.uploading || queueSummary.hashing" class="text-brand-400">
                ↑ {{ queueSummary.uploading }} uploading · # {{ queueSummary.hashing }} hashing
            </span>
            <span v-if="queue.done_count" class="text-emerald-400">✓ {{ queue.done_count }} uploaded</span>
            <span v-if="queue.duplicate_count" class="text-cyan-400">∾ {{ queue.duplicate_count }} already backed up</span>
            <span v-if="queue.error_count" class="text-red-400">! {{ queue.error_count }} error</span>
        </div>

        <!-- Status filter tabs. Client-side only; lets the user
             scope the activity feed to "what's running right now"
             vs. "what I uploaded today" vs. "what failed" without
             scrolling through thousands of cache-hit rows. -->
        <div
            v-if="queue.items.length"
            class="px-5 py-1.5 border-b border-white/[0.04] flex items-center gap-1 text-xs overflow-x-auto"
        >
            <button
                v-for="opt in ([
                    { v: 'all',          label: 'All' },
                    { v: 'in_progress',  label: 'In progress' },
                    { v: 'done',         label: 'Newly uploaded' },
                    { v: 'duplicate',    label: 'Already backed up' },
                    { v: 'error',        label: 'Errors' },
                ] as const)"
                :key="opt.v"
                class="px-2.5 py-1 rounded transition-colors whitespace-nowrap"
                :class="queueFilter === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                @click="queueFilter = opt.v"
            >{{ opt.label }}</button>
        </div>

        <!-- body. queue-scroll class widens the webkit scrollbar so
             it's grabbable with the mouse on Windows (default Tauri
             webview gives ~6px which is too thin). -->
        <div class="flex-1 overflow-auto queue-scroll">
            <div v-if="!queue.items.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🎬</div>
                    <div class="text-neutral-300 font-semibold">No demos yet</div>
                    <p class="text-sm text-neutral-500">
                        <template v-if="config.autoUploadRunning">
                            The launcher is watching your demos folder. Record a run and it will appear here.
                        </template>
                        <template v-else>
                            Turn on auto-upload to start watching your demos folder. New demos will appear here as they are backed up.
                        </template>
                    </p>
                </div>
            </div>
            <div
                v-else-if="!filteredQueueItems.length"
                class="h-full flex items-center justify-center p-8"
            >
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-neutral-300 font-semibold">Nothing matches this filter</div>
                    <p class="text-sm text-neutral-500">
                        Try switching to <button class="text-brand-400 hover:underline" @click="queueFilter = 'all'">All</button>.
                    </p>
                </div>
            </div>

            <ul v-else class="divide-y divide-white/[0.04]">
                <li
                    v-for="item in filteredQueueItems"
                    :key="item.path"
                    class="px-5 py-3"
                    @contextmenu="openContextMenu($event, item)"
                >
                    <button
                        class="w-full flex items-center gap-3 text-left"
                        @click="toggleExpand(item.path)"
                    >
                        <div class="flex-1 min-w-0">
                            <div class="text-sm text-neutral-100 truncate">{{ item.filename }}</div>
                            <div class="text-xs text-neutral-500 truncate">{{ item.path }}</div>
                        </div>
                        <div v-if="item.size_bytes" class="text-xs text-neutral-500">{{ formatBytes(item.size_bytes) }}</div>
                        <div class="text-xs font-semibold" :class="statusColor(item)">
                            {{ statusLabel(item) }}
                        </div>
                        <button
                            v-if="item.status === 'error' && config.autoUploadRunning"
                            class="text-[11px] px-2 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 font-semibold disabled:opacity-50"
                            :disabled="retrying.has(item.path)"
                            :title="`Re-queue ${item.filename} for upload`"
                            @click.stop="retryUpload(item)"
                        >{{ retrying.has(item.path) ? 'Retrying…' : 'Retry' }}</button>
                        <div class="text-xs text-neutral-600 w-3 text-right">
                            {{ expanded.has(item.path) ? '▾' : '▸' }}
                        </div>
                    </button>

                    <!-- expanded detail panel -->
                    <div v-if="expanded.has(item.path)" class="mt-3 ml-1 pl-3 border-l border-white/10 space-y-1.5 text-xs text-neutral-400">
                        <div v-if="duplicateExplain(item)" class="text-cyan-300/80">
                            {{ duplicateExplain(item) }}
                        </div>
                        <div v-if="item.error" class="text-red-300/90">
                            {{ item.error }}
                        </div>
                        <div class="flex flex-wrap gap-x-5 gap-y-1">
                            <span v-if="item.size_bytes">
                                <span class="text-neutral-500">Size:</span> {{ formatBytes(item.size_bytes) }}
                            </span>
                            <span v-if="item.hash_throughput_bps">
                                <span class="text-neutral-500">Hash:</span> {{ formatRate(item.hash_throughput_bps) }}
                            </span>
                            <span v-if="item.upload_throughput_bps">
                                <span class="text-neutral-500">Upload:</span> {{ formatRate(item.upload_throughput_bps) }}
                            </span>
                            <!-- Map name link, parsed from filename. Hidden
                                 when the filename doesn't match the standard
                                 <map>[physics]<time>(player).dm_* shape - we
                                 don't want to build a /maps/<garbage> URL. -->
                            <span v-if="mapNameFromFilename(item.filename)">
                                <span class="text-neutral-500">Map:</span>
                                <button
                                    class="ml-1 text-brand-400 hover:underline"
                                    @click.stop="openMapPage(item.filename)"
                                >{{ mapNameFromFilename(item.filename) }} ↗</button>
                            </span>
                            <!-- Demo id is the launcher's primary record of
                                 the upload; clicking goes to /demos/<id>/download
                                 because /demos/<id> alone is a 405 on the
                                 backend. Label says "Download" so the user
                                 knows exactly what the click will do. -->
                            <span v-if="item.demo_id">
                                <span class="text-neutral-500">Demo:</span>
                                <span class="ml-1">#{{ item.demo_id }}</span>
                                <button
                                    class="ml-2 text-brand-400 hover:underline"
                                    @click.stop="openDemoDownload(item.demo_id!)"
                                >Download ↓</button>
                            </span>
                        </div>
                    </div>
                </li>
            </ul>
        </div>

        <template v-if="ctxMenu">
            <div class="fixed inset-0 z-40" @click="closeContextMenu" @contextmenu.prevent="closeContextMenu"></div>
            <div
                class="fixed z-50 min-w-[180px] bg-neutral-900 border border-white/10 rounded shadow-xl py-1 text-sm"
                :style="{ left: ctxMenu.x + 'px', top: ctxMenu.y + 'px' }"
            >
                <button class="w-full text-left px-3 py-1.5 hover:bg-white/5 text-neutral-200" @click="ctxOpenInExplorer">Open in explorer</button>
                <button class="w-full text-left px-3 py-1.5 hover:bg-white/5 text-neutral-200" @click="ctxCopyPath">Copy path</button>
                <button
                    class="w-full text-left px-3 py-1.5 hover:bg-white/5 text-neutral-200"
                    title="Copy a /demo console command you can paste in Quake to play this demo"
                    @click="ctxCopyDemoCmd"
                >Copy /demo command</button>
                <div class="my-1 border-t border-white/10"></div>
                <button class="w-full text-left px-3 py-1.5 hover:bg-red-500/10 text-red-300" @click="ctxDeleteDemo">Delete demo</button>
            </div>
        </template>
    </div>
</template>

<style scoped>
/* Wider scrollbar for the queue. Tauri's Edge WebView2 defaults to a
 * ~6px overlay scrollbar that's hard to grab with the mouse - 12px is
 * the Windows Explorer default and feels right next to the row
 * density of the activity feed. Only applies to .queue-scroll so
 * other scrollable areas in the view keep the system default. */
.queue-scroll::-webkit-scrollbar { width: 12px; height: 12px; }
.queue-scroll::-webkit-scrollbar-track { background: rgba(255, 255, 255, 0.02); }
.queue-scroll::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.12);
    border-radius: 6px;
    border: 2px solid transparent;
    background-clip: content-box;
}
.queue-scroll::-webkit-scrollbar-thumb:hover {
    background: rgba(255, 255, 255, 0.22);
    background-clip: content-box;
}
</style>
