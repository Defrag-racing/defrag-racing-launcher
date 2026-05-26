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

    // defrag:// pending-connection prompt. Backend stashes the URL when
    // a deep link arrives and emits `deep-link://pending`; we read both
    // the live event AND the stashed value (cold-start case where the
    // webview mounts after the event already fired).
    type PendingDeepLink = { address: string; url: string };
    const pendingDeepLink = ref<PendingDeepLink | null>(null);
    const connectError = ref<string | null>(null);
    const connecting = ref(false);

    // Generic toast for connect errors / parse failures. Distinct from
    // pendingDeepLink because errors don't have a Connect button - the
    // URL was unusable.
    type DeepLinkError = { url: string; error: string };
    const deepLinkError = ref<DeepLinkError | null>(null);
    let deepLinkErrorTimer: number | undefined;

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
    let unlistenPending: UnlistenFn | null = null;
    let unlistenResult: UnlistenFn | null = null;
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
        unlistenPending = await listen<PendingDeepLink>('deep-link://pending', (ev) => {
            pendingDeepLink.value = ev.payload;
            connectError.value = null;
        });
        unlistenResult = await listen<{ ok: false; url: string; error: string }>(
            'deep-link://result',
            (ev) => {
                // Only error payloads ever land here now - success goes
                // through the user-confirmed `confirm_pending_deep_link`
                // command instead.
                if (!ev.payload.ok) {
                    deepLinkError.value = { url: ev.payload.url, error: ev.payload.error };
                    window.clearTimeout(deepLinkErrorTimer);
                    deepLinkErrorTimer = window.setTimeout(() => { deepLinkError.value = null; }, 6000);
                }
            },
        );

        // Cold-start case: deep-link plugin may have fired its event
        // before this component mounted. Pull the stashed value.
        try {
            const url = await tauri.getPendingDeepLink();
            if (url && !pendingDeepLink.value) {
                // Display the host:port portion of the URL while we
                // don't have a parsed address - same regex shape the
                // backend uses in protocol::parse_url.
                const m = url.match(/^defrag:\/\/([^/]+)/);
                pendingDeepLink.value = { url, address: m ? m[1] : url };
            }
        } catch { /* no-op */ }

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
        if (unlistenPending) unlistenPending();
        if (unlistenResult) unlistenResult();
        window.clearTimeout(deepLinkErrorTimer);
        if (updateCheckTimer !== undefined) window.clearInterval(updateCheckTimer);
        if (rateLimitPollTimer !== undefined) window.clearInterval(rateLimitPollTimer);
        if (nowTickTimer !== undefined) window.clearInterval(nowTickTimer);
    });

    const dismissDeepLinkError = () => {
        deepLinkError.value = null;
        window.clearTimeout(deepLinkErrorTimer);
    };

    const confirmConnect = async () => {
        connectError.value = null;
        connecting.value = true;
        try {
            await tauri.confirmPendingDeepLink();
            pendingDeepLink.value = null;
        } catch (e: any) {
            connectError.value = e?.toString?.() ?? 'Connect failed';
        } finally {
            connecting.value = false;
        }
    };

    const cancelConnect = async () => {
        await tauri.cancelPendingDeepLink();
        pendingDeepLink.value = null;
        connectError.value = null;
    };

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
                            Watching your demos folder. New <code class="bg-black/40 px-1 rounded">.dm_*</code> files
                            are hashed locally and uploaded to defrag.racing if the server doesn't already have them.
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

        <!-- defrag:// pending-connection prompt: user-explicit confirm -->
        <div
            v-if="pendingDeepLink"
            class="px-5 py-3 border-b border-brand-500/20 bg-brand-500/10 text-sm flex items-center gap-3"
        >
            <div class="flex-1 min-w-0">
                <div class="text-brand-300 font-semibold">Connect to <span class="font-mono">{{ pendingDeepLink.address }}</span>?</div>
                <div v-if="connectError" class="text-xs text-red-300 mt-0.5">{{ connectError }}</div>
            </div>
            <button
                class="px-3 py-1.5 rounded bg-brand-500/30 hover:bg-brand-500/40 text-brand-200 font-semibold disabled:opacity-50"
                :disabled="connecting"
                @click="confirmConnect"
            >Connect</button>
            <button
                class="px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300"
                @click="cancelConnect"
            >Dismiss</button>
        </div>

        <div
            v-if="deepLinkError"
            class="px-5 py-2 border-b text-xs flex items-center gap-2 bg-red-500/10 border-red-500/20 text-red-300"
        >
            <span>
                Couldn't open <code class="font-mono">{{ deepLinkError.url }}</code> - {{ deepLinkError.error }}
            </span>
            <button class="ml-auto text-neutral-400 hover:text-neutral-200" @click="dismissDeepLinkError">×</button>
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

        <!-- body -->
        <div class="flex-1 overflow-auto">
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

            <ul v-else class="divide-y divide-white/[0.04]">
                <li v-for="item in queue.items" :key="item.path" class="px-5 py-3">
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
    </div>
</template>
