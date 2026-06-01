<script setup lang="ts">
    // Unified "Demos" view. One list of every .dm_* on disk, with the
    // live auto-backup status (hashing / uploading / done / error) and a
    // YouTube Render button overlaid on the same row. Replaces the old
    // split between the Dashboard (live session queue) and the Library
    // (full on-disk catalog) - the user thinks "my demos", not "this
    // session vs. the whole folder", so it's one list.
    //
    // Data model:
    //   - `demos`     : full on-disk catalog (list_demos), the row source.
    //   - `queue`     : the watcher's live session queue (event-driven).
    //                   Overlaid onto matching rows by path so a row that
    //                   is being hashed/uploaded right now shows that
    //                   instead of its persisted cache status.
    //   - `renderState`: per-hash render status, lazily warmed + updated
    //                   on Render click.

    import { computed, onActivated, onMounted, onUnmounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { getVersion } from '@tauri-apps/api/app';
    import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
    import {
        tauri,
        type UploadStateSnapshot,
        type PendingUpload,
        type DemoLibraryEntry,
        type RenderStatusResponse,
    } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { useUpdaterStore } from '../stores/updater';
    import { fetchChangelogSince, renderMarkdown, type ChangelogSection } from '../lib/changelog';
    import DemosFolderChip from '../components/DemosFolderChip.vue';

    const router = useRouter();
    const config = useConfigStore();
    const updater = useUpdaterStore();

    // -- live session queue -------------------------------------------
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

    // Live items keyed by path so a row can find its in-flight state in
    // O(1) while rendering the (potentially huge) disk catalog.
    const queueByPath = computed(() => {
        const m = new Map<string, PendingUpload>();
        for (const it of queue.value.items) m.set(it.path, it);
        return m;
    });

    // -- on-disk catalog ----------------------------------------------
    const demos = ref<DemoLibraryEntry[]>([]);
    const listLoading = ref(false);
    const listError = ref<string | null>(null);

    const refreshList = async () => {
        listLoading.value = true;
        listError.value = null;
        try {
            demos.value = await tauri.listDemos();
        } catch (e: any) {
            listError.value = e?.toString?.() ?? 'Failed to list demos';
        } finally {
            listLoading.value = false;
        }
    };

    // Debounced re-list after the watcher finishes work, so a freshly
    // uploaded demo picks up its hash (needed for Render) + cache status
    // without the user clicking Refresh. Cheap to skip when nothing new
    // reached a terminal state.
    let relistTimer: number | undefined;
    let lastTerminalCount = 0;
    const scheduleRelist = () => {
        if (relistTimer !== undefined) return;
        relistTimer = window.setTimeout(() => {
            relistTimer = undefined;
            void refreshList();
        }, 3000);
    };

    // -- render state -------------------------------------------------
    const renderState = ref<Record<string, RenderStatusResponse>>({});
    const rendering = ref<Set<string>>(new Set());
    let warmupCancelled = false;

    const warmupRenderStates = async () => {
        if (!config.hasToken) return;
        const sample = demos.value
            .filter((d) => d.hash && d.upload_status)
            .slice(0, 100);
        for (const d of sample) {
            if (warmupCancelled) return;
            try {
                const s = await tauri.getRenderStatus(d.hash!);
                renderState.value = { ...renderState.value, [d.hash!]: s };
            } catch {
                // ignore - real errors surface on click
            }
        }
    };

    // -- search / sort / filter ---------------------------------------
    type SortKey = 'date_desc' | 'date_asc' | 'name_asc' | 'name_desc';
    const sortKey = ref<SortKey>('date_desc');
    const search = ref('');

    type RowFilter = 'all' | 'in_progress' | 'uploaded' | 'not_uploaded' | 'error';
    const rowFilter = ref<RowFilter>('all');

    type StatusKind = 'inprogress' | 'done' | 'duplicate' | 'error' | 'none';
    interface RowStatus { label: string; color: string; kind: StatusKind }

    // Resolve the status shown on a row: the live queue wins (it's
    // happening right now) and otherwise we fall back to the persisted
    // cache status from the disk listing.
    const resolveStatus = (d: DemoLibraryEntry): RowStatus => {
        const live = queueByPath.value.get(d.path);
        if (live) {
            switch (live.status) {
                case 'pending':   return { label: 'Waiting',           color: 'text-neutral-400', kind: 'inprogress' };
                case 'hashing':   return { label: 'Hashing…',          color: 'text-brand-400',   kind: 'inprogress' };
                case 'uploading': return { label: 'Uploading…',        color: 'text-brand-400',   kind: 'inprogress' };
                case 'done':      return { label: 'Uploaded',          color: 'text-emerald-400', kind: 'done' };
                case 'duplicate': return { label: 'Already backed up', color: 'text-cyan-400',    kind: 'duplicate' };
                case 'error':     return { label: 'Error',             color: 'text-red-400',     kind: 'error' };
            }
        }
        if (d.upload_status === 'done')      return { label: 'Backed up',         color: 'text-emerald-400/80', kind: 'done' };
        if (d.upload_status === 'duplicate') return { label: 'Already backed up', color: 'text-cyan-400/80',    kind: 'duplicate' };
        return { label: 'Not uploaded', color: 'text-neutral-500', kind: 'none' };
    };

    // Union of the disk catalog + any live queue item not yet on disk
    // (a brand-new recording the watcher just saw, before the next
    // re-list). Synthetic rows get a now-ish mtime so they float to the
    // top under "Newest".
    const allRows = computed<DemoLibraryEntry[]>(() => {
        const byPath = new Map<string, DemoLibraryEntry>();
        for (const d of demos.value) byPath.set(d.path, d);
        const nowSec = Math.floor(Date.now() / 1000);
        for (const it of queue.value.items) {
            if (byPath.has(it.path)) continue;
            byPath.set(it.path, {
                path: it.path,
                filename: it.filename,
                size_bytes: it.size_bytes ?? 0,
                mtime: nowSec,
                hash: null,
                demo_id: it.demo_id,
                upload_status: null,
            });
        }
        return Array.from(byPath.values());
    });

    const filteredDemos = computed(() => {
        const q = search.value.trim().toLowerCase();
        let result = allRows.value.slice();
        if (q) result = result.filter((d) => d.filename.toLowerCase().includes(q));
        if (rowFilter.value !== 'all') {
            result = result.filter((d) => {
                const k = resolveStatus(d).kind;
                switch (rowFilter.value) {
                    case 'in_progress':  return k === 'inprogress';
                    case 'uploaded':     return k === 'done' || k === 'duplicate';
                    case 'not_uploaded': return k === 'none';
                    case 'error':        return k === 'error';
                }
            });
        }
        result.sort((a, b) => {
            switch (sortKey.value) {
                case 'date_desc': return b.mtime - a.mtime;
                case 'date_asc':  return a.mtime - b.mtime;
                case 'name_asc':  return a.filename.localeCompare(b.filename);
                case 'name_desc': return b.filename.localeCompare(a.filename);
            }
        });
        return result;
    });

    // -- CPU throttle / speed -----------------------------------------
    const currentThrottlePct = ref(15);
    const refreshThrottle = async () => {
        try { currentThrottlePct.value = await tauri.getCpuThrottlePct(); } catch { /* watcher off */ }
    };
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
        const idx = cycle.findIndex((t) => t === currentThrottlePct.value);
        const base = idx >= 0 ? idx : 0;
        return cycle[(base + 1) % cycle.length];
    });
    const willWrapToSaved = computed(() => {
        const saved = config.config.cpu_throttle_pct ?? 15;
        return nextSpeedTier.value === saved && currentThrottlePct.value !== saved;
    });
    const speedButtonText = computed(() => {
        const next = nextSpeedTier.value;
        if (willWrapToSaved.value) return `Slow down (${next}%)`;
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

    // -- what's new ----------------------------------------------------
    const whatsNewOpen = ref(false);
    const whatsNewLoading = ref(false);
    const whatsNewError = ref<string | null>(null);
    const whatsNewSections = ref<ChangelogSection[]>([]);
    const whatsNewInstalled = ref<string>('');
    const renderedBody = (body: string) => renderMarkdown(body);
    const toggleWhatsNew = async () => {
        if (whatsNewOpen.value) { whatsNewOpen.value = false; return; }
        whatsNewOpen.value = true;
        if (whatsNewSections.value.length > 0) return;
        whatsNewLoading.value = true;
        whatsNewError.value = null;
        try {
            const installed = await getVersion();
            whatsNewInstalled.value = installed;
            whatsNewSections.value = await fetchChangelogSince(installed);
        } catch (e: any) {
            whatsNewError.value = e?.toString?.() ?? 'Failed to load changelog';
        } finally {
            whatsNewLoading.value = false;
        }
    };

    // -- rate-limit countdown -----------------------------------------
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
        try { rateLimitResumeAtMs.value = await tauri.getRateLimitResumeAtMs(); } catch { /* watcher off */ }
    };

    // -- lifecycle -----------------------------------------------------
    let unlisten: UnlistenFn | null = null;
    const refreshPaused = async () => {
        try { paused.value = await tauri.isAutoUploadPaused(); } catch { paused.value = false; }
    };

    onMounted(async () => {
        queue.value = await tauri.getUploadState();
        lastTerminalCount = queue.value.processed_count;
        await refreshPaused();
        await refreshThrottle();
        await pollRateLimit();
        await refreshList();
        void warmupRenderStates();

        rateLimitPollTimer = window.setInterval(pollRateLimit, 1000);
        nowTickTimer = window.setInterval(() => { nowMs.value = Date.now(); }, 250);

        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            queue.value = ev.payload;
            // Something finished hashing/uploading -> re-list soon so the
            // row picks up its hash + persisted status.
            if (ev.payload.processed_count > lastTerminalCount) {
                lastTerminalCount = ev.payload.processed_count;
                scheduleRelist();
            }
        });
    });

    // KeepAlive caches this view across tab switches, so onMounted runs
    // only once. Re-list on re-entry to pick up demos recorded / deleted
    // while the user was on another tab (the event-driven relist only
    // covers watcher activity). Skip the first activation - onMounted
    // already listed - so we don't double-list on initial load.
    let activatedOnce = false;
    onActivated(() => {
        if (!activatedOnce) { activatedOnce = true; return; }
        void refreshList();
    });

    onUnmounted(() => {
        warmupCancelled = true;
        if (unlisten) unlisten();
        if (rateLimitPollTimer !== undefined) window.clearInterval(rateLimitPollTimer);
        if (nowTickTimer !== undefined) window.clearInterval(nowTickTimer);
        if (relistTimer !== undefined) window.clearTimeout(relistTimer);
    });

    const installUpdate = () => updater.install();

    const toggle = async () => {
        toggleError.value = null;
        toggling.value = true;
        try {
            if (config.autoUploadRunning) await tauri.stopAutoUpload();
            else await tauri.startAutoUpload();
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

    // -- formatting helpers -------------------------------------------
    const formatBytes = (n: number | null) => {
        if (n == null || n <= 0) return '-';
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
        if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
        return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
    };
    const formatDate = (epochSec: number): string => {
        if (!epochSec) return '';
        return new Date(epochSec * 1000).toLocaleString();
    };

    const mapNameFromFilename = (filename: string): string | null => {
        const idx = filename.indexOf('[');
        if (idx <= 0) return null;
        const name = filename.slice(0, idx).trim();
        return name.length > 0 ? name : null;
    };
    const openMapPage = (filename: string) => {
        const map = mapNameFromFilename(filename);
        if (map) openUrl(`https://defrag.racing/maps/${encodeURIComponent(map)}`).catch(() => {});
    };

    // -- render --------------------------------------------------------
    const renderDemo = async (d: DemoLibraryEntry) => {
        if (!d.hash) {
            toggleError.value = `${d.filename} hasn't been backed up yet - turn on auto-backup and the watcher will upload it first.`;
            return;
        }
        rendering.value.add(d.hash);
        rendering.value = new Set(rendering.value);
        toggleError.value = null;
        try {
            const r = await tauri.requestRender(d.hash);
            switch (r._http_status) {
                case 200:
                    renderState.value = {
                        ...renderState.value,
                        [d.hash]: {
                            has_render: true,
                            id: r.id,
                            status: r.status,
                            youtube_url: r.youtube_url ?? null,
                            youtube_video_id: r.youtube_video_id ?? null,
                        },
                    };
                    break;
                case 401:
                    toggleError.value = 'Render rejected - your launcher token is invalid, expired, or was revoked. Open Settings, remove the token and paste a freshly generated one.';
                    break;
                case 404:
                    toggleError.value = 'Demo not yet uploaded to the server. The watcher will pick it up shortly - try again in a moment.';
                    break;
                case 429:
                    toggleError.value = `Daily render quota reached (${r.error ?? '20/day'}).`;
                    break;
                case 403:
                    toggleError.value = r.error ?? 'Your account is restricted from rendering.';
                    break;
                default:
                    toggleError.value = r.error ?? `Unexpected response (HTTP ${r._http_status}).`;
            }
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Render request failed';
        } finally {
            rendering.value.delete(d.hash);
            rendering.value = new Set(rendering.value);
        }
    };

    const renderLabelFor = (d: DemoLibraryEntry): string => {
        if (!d.hash) return 'Render';
        if (rendering.value.has(d.hash)) return 'Working…';
        const st = renderState.value[d.hash];
        if (!st || !st.has_render) return 'Render';
        switch (st.status) {
            case 'completed': return 'View ▶';
            case 'pending':   return 'Queued';
            case 'rendering': return 'Rendering…';
            case 'uploading': return 'Uploading…';
            case 'failed':    return 'Retry';
            default:          return 'Queued';
        }
    };
    const renderIsCompleted = (d: DemoLibraryEntry): boolean =>
        !!d.hash && renderState.value[d.hash]?.status === 'completed';

    const renderClickFor = (d: DemoLibraryEntry) => {
        if (!d.hash) return renderDemo(d);
        const st = renderState.value[d.hash];
        if (st?.status === 'completed' && st.youtube_url) {
            return openUrl(st.youtube_url).catch(() => {});
        }
        if (st && (st.status === 'pending' || st.status === 'rendering' || st.status === 'uploading')) return;
        return renderDemo(d);
    };

    // -- retry / context menu -----------------------------------------
    const retrying = ref<Set<string>>(new Set());
    const retryUpload = async (path: string) => {
        if (retrying.value.has(path)) return;
        retrying.value.add(path);
        retrying.value = new Set(retrying.value);
        try {
            await tauri.retryUpload(path);
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Retry failed';
        } finally {
            retrying.value.delete(path);
            retrying.value = new Set(retrying.value);
        }
    };

    type CtxMenu = { x: number; y: number; demo: DemoLibraryEntry };
    const ctxMenu = ref<CtxMenu | null>(null);
    const openContextMenu = (e: MouseEvent, d: DemoLibraryEntry) => {
        e.preventDefault();
        ctxMenu.value = { x: e.clientX, y: e.clientY, demo: d };
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
        const p = ctxMenu.value.demo.path;
        closeContextMenu();
        try { await revealItemInDir(p); } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Failed to open in explorer';
        }
    };
    const ctxCopyPath = async () => {
        if (!ctxMenu.value) return;
        await copyToClipboard(ctxMenu.value.demo.path);
        closeContextMenu();
    };
    const ctxCopyDemoCmd = async () => {
        if (!ctxMenu.value) return;
        await copyToClipboard(`/demo ${filenameStem(ctxMenu.value.demo.filename)}`);
        closeContextMenu();
    };
    const ctxDeleteDemo = async () => {
        if (!ctxMenu.value) return;
        const d = ctxMenu.value.demo;
        closeContextMenu();
        if (!window.confirm(`Permanently delete ${d.filename}?\n\nThe file on disk will be removed. Anything already on defrag.racing stays.`)) return;
        try {
            await tauri.deleteDemo(d.path);
            demos.value = demos.value.filter((x) => x.path !== d.path);
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Delete failed';
        }
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <!-- top bar: auto-backup status + controls + folder chip -->
        <header class="px-5 py-3 border-b border-white/10 flex items-start justify-between gap-3">
            <div class="flex items-start gap-2 min-w-0">
                <div
                    class="w-2 h-2 rounded-full mt-1.5 flex-shrink-0"
                    :class="!config.autoUploadRunning ? 'bg-neutral-600' : (paused ? 'bg-amber-400' : 'bg-emerald-400')"
                ></div>
                <div class="text-sm min-w-0">
                    <div>
                        <span class="font-semibold">Auto-backup</span>
                        <span class="text-neutral-500 ml-1">
                            {{ !config.autoUploadRunning ? 'off' : (paused ? 'paused' : 'running') }}
                        </span>
                    </div>
                    <div class="text-xs text-neutral-500 mt-0.5 leading-snug">
                        <template v-if="config.autoUploadRunning && !paused">
                            Watching your demos folder live - new demos are backed up to defrag.racing within ~30s and show up in the list below. Render any one to YouTube.
                        </template>
                        <template v-else-if="config.autoUploadRunning && paused">
                            Watcher is still picking up new demos, but uploads are paused. Click Resume to drain the queue.
                        </template>
                        <template v-else>
                            Click <strong class="text-brand-400">Start</strong> to back up new demos automatically. Your existing demos are listed below regardless.
                        </template>
                    </div>
                    <DemosFolderChip class="mt-1.5" />
                </div>
            </div>
            <div class="flex items-center gap-2 flex-shrink-0">
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
                >{{ config.autoUploadRunning ? 'Stop' : 'Start' }}</button>
            </div>
        </header>

        <p v-if="toggleError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ toggleError }}
        </p>
        <p v-if="listError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ listError }}
        </p>

        <!-- rate-limit countdown -->
        <div
            v-if="isRateLimited"
            class="px-5 py-2 border-b border-amber-500/20 bg-amber-500/10 text-xs text-amber-200 flex items-center gap-2"
        >
            <span class="text-amber-400">⏳</span>
            <span>
                Rate-limited by defrag.racing - resuming in
                <strong>{{ rateLimitSecondsLeft }}s</strong>. The launcher will retry automatically.
            </span>
        </div>

        <!-- no-token banner -->
        <div
            v-if="config.loaded && !config.hasToken"
            class="px-5 py-3 border-b border-amber-500/30 bg-amber-500/10 text-xs text-amber-100"
        >
            <div class="font-semibold text-amber-200 mb-1">
                No token saved - most launcher features are disabled
            </div>
            <ul class="space-y-0.5 pl-1 mb-2">
                <li>• Auto-backup of recorded demos</li>
                <li>• YouTube renders straight from this list</li>
                <li>• Server browser with your PB and rank per map</li>
            </ul>
            <div class="flex items-center justify-between gap-2">
                <span class="text-amber-100/80">
                    Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links work without one.
                </span>
                <button
                    class="px-3 py-1 rounded bg-amber-500/30 hover:bg-amber-500/40 text-amber-100 font-semibold flex-shrink-0"
                    @click="router.push({ name: 'settings' })"
                >Add token →</button>
            </div>
        </div>

        <!-- update banner + what's new -->
        <div
            v-if="updater.state.kind === 'available'"
            class="border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300"
        >
            <div class="px-5 py-2 flex items-center gap-3">
                <span>Update <strong>v{{ updater.state.version }}</strong> is available.</span>
                <button class="ml-auto px-2 py-0.5 rounded bg-white/5 hover:bg-white/10" @click="toggleWhatsNew">
                    {{ whatsNewOpen ? 'Hide changes' : 'View changes' }}
                </button>
                <button class="px-2 py-0.5 rounded bg-brand-500/20 hover:bg-brand-500/30 font-semibold" @click="installUpdate">
                    Install &amp; restart
                </button>
            </div>
            <div v-if="whatsNewOpen" class="px-5 py-3 border-t border-brand-500/20 bg-black/30 max-h-72 overflow-y-auto">
                <div v-if="whatsNewLoading" class="text-neutral-400">Loading changelog…</div>
                <div v-else-if="whatsNewError" class="text-red-300">{{ whatsNewError }}</div>
                <div v-else-if="whatsNewSections.length === 0" class="text-neutral-400">
                    Nothing newer than v{{ whatsNewInstalled }} in the changelog yet.
                </div>
                <div v-else class="space-y-4">
                    <section v-for="s in whatsNewSections" :key="s.version">
                        <h3 class="text-sm font-semibold text-brand-200 mb-1">v{{ s.version }}</h3>
                        <div class="text-xs text-neutral-200" v-html="renderedBody(s.body)"></div>
                    </section>
                </div>
            </div>
        </div>
        <div v-else-if="updater.state.kind === 'downloading'" class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300">
            Downloading update… {{ updater.state.percent }}%
        </div>
        <div v-else-if="updater.state.kind === 'installing'" class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300">
            Installing… the launcher will restart in a moment.
        </div>
        <div v-else-if="updater.state.kind === 'error'" class="px-5 py-2 border-b border-red-500/20 bg-red-500/10 text-xs text-red-300">
            Update failed: {{ updater.state.message }}
        </div>

        <!-- session summary strip -->
        <div
            v-if="queue.processed_count"
            class="px-5 py-2 border-b border-white/[0.04] text-xs text-neutral-400 flex items-center gap-3 flex-wrap"
        >
            <span class="text-neutral-200 font-semibold">{{ queue.processed_count }} processed this session</span>
            <span v-if="queue.done_count" class="text-emerald-400">✓ {{ queue.done_count }} uploaded</span>
            <span v-if="queue.duplicate_count" class="text-cyan-400">∾ {{ queue.duplicate_count }} already backed up</span>
            <span v-if="queue.error_count" class="text-red-400">! {{ queue.error_count }} error</span>
        </div>

        <!-- search + sort + filter -->
        <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 text-xs flex-wrap">
            <input
                v-model="search"
                type="text"
                placeholder="Search filename…"
                class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
            />
            <div class="flex bg-white/5 rounded overflow-hidden">
                <button
                    v-for="opt in ([
                        { v: 'all',          label: 'All' },
                        { v: 'in_progress',  label: 'In progress' },
                        { v: 'uploaded',     label: 'Backed up' },
                        { v: 'not_uploaded', label: 'Not uploaded' },
                        { v: 'error',        label: 'Errors' },
                    ] as const)"
                    :key="opt.v"
                    class="px-2.5 py-1 transition-colors whitespace-nowrap"
                    :class="rowFilter === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200'"
                    @click="rowFilter = opt.v"
                >{{ opt.label }}</button>
            </div>
            <span class="text-neutral-500 mx-1">Sort:</span>
            <div class="flex bg-white/5 rounded overflow-hidden">
                <button
                    v-for="opt in ([
                        { v: 'date_desc', label: 'Newest' },
                        { v: 'date_asc',  label: 'Oldest' },
                        { v: 'name_asc',  label: 'A→Z' },
                        { v: 'name_desc', label: 'Z→A' },
                    ] as const)"
                    :key="opt.v"
                    class="px-2.5 py-1 transition-colors"
                    :class="sortKey === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200'"
                    @click="sortKey = opt.v"
                >{{ opt.label }}</button>
            </div>
            <button
                class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                :disabled="listLoading"
                @click="refreshList"
            >{{ listLoading ? 'Loading…' : 'Refresh' }}</button>
            <span class="text-neutral-500 ml-auto">{{ filteredDemos.length }} / {{ allRows.length }}</span>
        </div>

        <!-- the list -->
        <div class="flex-1 overflow-auto queue-scroll">
            <div v-if="listLoading && !allRows.length" class="p-8 text-center text-sm text-neutral-500">
                Listing demos…
            </div>
            <div v-else-if="!allRows.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🎬</div>
                    <div class="text-neutral-300 font-semibold">No demos yet</div>
                    <p class="text-sm text-neutral-500">
                        <template v-if="config.autoUploadRunning">
                            The launcher is watching your demos folder. Record a run and it will appear here.
                        </template>
                        <template v-else>
                            Set your demos folder in Settings, then turn on auto-backup. Your demos will appear here.
                        </template>
                    </p>
                </div>
            </div>
            <div v-else-if="!filteredDemos.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-neutral-300 font-semibold">Nothing matches</div>
                    <p class="text-sm text-neutral-500">
                        <button class="text-brand-400 hover:underline" @click="search = ''; rowFilter = 'all'">Clear search &amp; filters</button>.
                    </p>
                </div>
            </div>
            <ul v-else class="divide-y divide-white/[0.04]">
                <li
                    v-for="d in filteredDemos"
                    :key="d.path"
                    class="px-5 py-2 flex items-center gap-3 hover:bg-white/[0.02]"
                    @contextmenu="openContextMenu($event, d)"
                >
                    <div class="flex-1 min-w-0">
                        <div class="text-sm text-neutral-200 truncate font-medium" :title="d.filename">{{ d.filename }}</div>
                        <div class="text-xs text-neutral-500 truncate flex items-center gap-2 mt-0.5">
                            <button
                                v-if="mapNameFromFilename(d.filename)"
                                class="text-brand-400 hover:underline"
                                @click="openMapPage(d.filename)"
                            >{{ mapNameFromFilename(d.filename) }}</button>
                            <span class="text-neutral-600" v-if="mapNameFromFilename(d.filename)">·</span>
                            <span>{{ formatBytes(d.size_bytes) }}</span>
                            <template v-if="d.mtime">
                                <span class="text-neutral-600">·</span>
                                <span>{{ formatDate(d.mtime) }}</span>
                            </template>
                        </div>
                    </div>

                    <!-- live / persisted status -->
                    <div class="text-xs font-semibold flex-shrink-0" :class="resolveStatus(d).color">
                        {{ resolveStatus(d).label }}
                    </div>

                    <!-- retry when the live upload errored -->
                    <button
                        v-if="resolveStatus(d).kind === 'error' && config.autoUploadRunning"
                        class="text-[11px] px-2 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 font-semibold disabled:opacity-50 flex-shrink-0"
                        :disabled="retrying.has(d.path)"
                        :title="`Re-queue ${d.filename} for upload`"
                        @click.stop="retryUpload(d.path)"
                    >{{ retrying.has(d.path) ? 'Retrying…' : 'Retry' }}</button>

                    <!-- render -->
                    <button
                        v-if="config.hasToken"
                        class="px-3 py-1 rounded text-xs font-semibold disabled:opacity-50 flex-shrink-0"
                        :class="renderIsCompleted(d)
                            ? 'bg-red-500/20 hover:bg-red-500/30 text-red-300'
                            : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-300'"
                        :disabled="!d.hash || (!!d.hash && rendering.has(d.hash))"
                        :title="!d.hash ? 'Back this demo up first (auto-backup uploads it), then you can render it' : 'Queue a YouTube render'"
                        @click="renderClickFor(d)"
                    >{{ renderLabelFor(d) }}</button>
                </li>
            </ul>
        </div>

        <!-- context menu -->
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
/* Wider scrollbar for the list. Tauri's Edge WebView2 defaults to a
 * ~6px overlay scrollbar that's hard to grab with the mouse. */
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
