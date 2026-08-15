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

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { revealItemInDir } from '@tauri-apps/plugin-opener';
    import { openExternal } from '../lib/open';
    import { LazyStore } from '@tauri-apps/plugin-store';
    import {
        tauri,
        type UploadStateSnapshot,
        type PendingUpload,
        type DemoLibraryEntry,
        type RenderStatusResponse,
        type DemoAssocStatus,
    } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import DemosFolderChip from '../components/DemosFolderChip.vue';
    import TokenFeatureList from '../components/TokenFeatureList.vue';
    import TokenFreeFeatures from '../components/TokenFreeFeatures.vue';
    import DemoPlayerPanel, { type PlayTarget } from '../components/DemoPlayerPanel.vue';

    const router = useRouter();
    const config = useConfigStore();

    // Embedded demo player / comparison run on Windows and Linux (X11 / XWayland).
    const isEmbedSupported =
        navigator.userAgent.includes('Windows') || navigator.userAgent.includes('Linux');

    // Embedded player overlay: set to a demo to cover the Demos section with the
    // player; the panel's close button clears it.
    const playerTarget = ref<PlayTarget | null>(null);

    // Before the embedded player loads a demo we make sure the map it needs is
    // installed - same idea as the Maps tab: check the engine's game dirs and
    // download the pk3 if it's missing, otherwise the demo plays into a black
    // "CLIENT/SERVER GAME MISMATCH" / wrong-map screen. `preparingMap` drives a
    // small "Preparing map..." overlay; `mapError` opens a popup telling the
    // user to grab the map manually when the auto-download fails.
    const preparingMap = ref<string | null>(null);
    const mapError = ref<{ map: string; detail: string } | null>(null);
    // Live download progress for the map being prepared (from the backend's
    // `demo-map-progress` event): phase "checking" (scanning installed maps) or
    // "downloading" with byte counts.
    const mapProgress = ref<{ phase: string; received: number; total: number | null } | null>(null);
    const mapPercent = computed(() => {
        const p = mapProgress.value;
        if (!p || p.phase !== 'downloading' || !p.total) return null;
        return Math.min(100, Math.round((p.received / p.total) * 100));
    });
    const fmtMB = (bytes: number) => `${(bytes / (1024 * 1024)).toFixed(1)} MB`;

    /** Ensure every demo's map is installed (downloading missing pk3s). Each
     *  item carries the demo path (so the backend knows where this demo's
     *  content lives) and its map name. Deduped by map name. Resolves true on
     *  success; on failure sets `mapError` and resolves false so the caller
     *  skips opening the player. */
    const ensureMaps = async (
        items: { path: string; map: string | null }[],
    ): Promise<boolean> => {
        // One download per distinct map (first demo path wins).
        const byMap = new Map<string, string>();
        for (const it of items) if (it.map && !byMap.has(it.map)) byMap.set(it.map, it.path);
        if (byMap.size === 0) return true; // unknown map name -> let the engine try
        try {
            for (const [map, path] of byMap) {
                preparingMap.value = map;
                mapProgress.value = { phase: 'checking', received: 0, total: null };
                await tauri.ensureDemoMap(path, map);
            }
            return true;
        } catch (e) {
            mapError.value = {
                map: [...byMap.keys()].join(', '),
                detail: e instanceof Error ? e.message : String(e),
            };
            return false;
        } finally {
            preparingMap.value = null;
            mapProgress.value = null;
        }
    };

    // ---- a demo handed over by the file manager ---------------------
    // "Play in Defrag Launcher" on a .dm_68, or a double-click once the user
    // has made us the default. The file is usually NOT in a demos folder -
    // Downloads, the Desktop, a Discord attachment - so it is staged into a
    // folder the engine can load from before it plays.
    const openExternalDemo = async (path: string) => {
        try {
            const staged = await tauri.stageDemo(path);
            const filename = path.split(/[\\/]/).pop() ?? 'demo.dm_68';

            router.push({ name: 'dashboard' });
            compareTarget.value = null;

            if (!(await ensureMaps([{ path: staged, map: mapNameFromFilename(filename) }]))) return;

            playerTarget.value = { path: staged, name: filename };
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Could not open that demo';
        }
    };

    const playDemo = async (d: DemoLibraryEntry) => {
        compareTarget.value = null;
        if (!(await ensureMaps([{ path: d.path, map: mapNameFromFilename(d.filename) }]))) return;
        playerTarget.value = { path: d.path, name: d.filename };
    };

    // -- side-by-side comparison (premium, token-gated) ---------------
    // 2-4 demos play in their own engines, tiled and locked together. Selection
    // is two-step: click Compare on the first demo -> the list enters pick mode
    // (same-map demos float to the top) -> Add up to 4 -> Start compare.
    const MAX_COMPARE = 4;
    const compareTarget = ref<{ demos: PlayTarget[] } | null>(null);
    const compareSel = ref<DemoLibraryEntry[]>([]);
    const compareMapKey = computed(() =>
        compareSel.value.length
            ? (mapNameFromFilename(compareSel.value[0].filename) ?? '').toLowerCase()
            : '',
    );
    // Index of `d` in the current selection (-1 = not selected). Doubles as the
    // pane letter (0=A, 1=B, ...).
    const compareIndexOf = (d: DemoLibraryEntry) =>
        compareSel.value.findIndex((x) => x.path === d.path);
    // Begin picking: seed the selection with the first demo, enter pick mode.
    const startComparePick = (d: DemoLibraryEntry) => {
        if (!config.hasToken) return; // premium
        compareSel.value = [d];
    };
    const cancelComparePick = () => {
        compareSel.value = [];
    };
    // Add/remove a demo from the selection (capped at MAX_COMPARE).
    const toggleCompareSel = (d: DemoLibraryEntry) => {
        const i = compareIndexOf(d);
        if (i >= 0) compareSel.value.splice(i, 1);
        else if (compareSel.value.length < MAX_COMPARE) compareSel.value.push(d);
    };
    // Launch the comparison with the picked demos.
    const launchCompare = async () => {
        if (compareSel.value.length < 2) return;
        const items = compareSel.value.map((d) => ({
            path: d.path,
            map: mapNameFromFilename(d.filename),
        }));
        if (!(await ensureMaps(items))) return;
        playerTarget.value = null;
        compareTarget.value = {
            demos: compareSel.value.map((d) => ({ path: d.path, name: d.filename })),
        };
        compareSel.value = [];
    };
    const compareLetter = (i: number) => String.fromCharCode(65 + i);
    // True when `d` shares the first pick's map (same-map comparisons are the
    // useful case, so they're surfaced first and the rest is de-emphasized).
    const isSameMapAsA = (d: DemoLibraryEntry) =>
        !!compareMapKey.value &&
        (mapNameFromFilename(d.filename) ?? '').toLowerCase() === compareMapKey.value;

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
    // renderState maps a demo's file hash -> its render status. Two sources
    // feed it: (1) the bulk "rendered index" reconcile, which fills every
    // COMPLETED render in one call (cached in a plugin-store file so a restart
    // starts from a cheap delta, not a full re-pull), and (2) a small
    // in-progress poll for the handful of renders queued this session, so
    // they flip to "View ▶" live. Replaces the old warmup that fired up to
    // 100 per-row render-status calls on every launch.
    const renderState = ref<Record<string, RenderStatusResponse>>({});
    const rendering = ref<Set<string>>(new Set());

    // Persisted in a plugin-store JSON file (appDataDir), NOT localStorage:
    // the webview's localStorage was not surviving restarts, so the YouTube
    // links were re-fetched from scratch every launch. The store file is the
    // same mechanism the rest of the app relies on for durable state.
    const indexStore = new LazyStore('render-index.json');
    const MAP_FIELD = 'map';          // { hash: video_id }
    const CURSOR_FIELD = 'cursor';    // unix ts cursor
    const SYNC_COOLDOWN_MS = 30_000;  // anti-spam for rapid tab toggling
    let lastIndexSyncMs = 0;
    let indexSyncing = false;
    let active = false; // is the Demos tab the active view (KeepAlive)

    const readIndexMap = async (): Promise<Record<string, string>> => {
        try { return (await indexStore.get<Record<string, string>>(MAP_FIELD)) ?? {}; } catch { return {}; }
    };
    const writeIndexMap = async (m: Record<string, string>) => {
        try { await indexStore.set(MAP_FIELD, m); await indexStore.save(); } catch { /* non-fatal */ }
    };

    const completedEntry = (videoId: string): RenderStatusResponse => ({
        has_render: true,
        status: 'completed',
        youtube_video_id: videoId,
        youtube_url: `https://www.youtube.com/watch?v=${videoId}`,
    });

    // Instant: paint completed renders from the persisted cache before any
    // network call, so the list shows "View ▶" the moment Demos opens.
    // Builds the whole map in one plain object and assigns renderState ONCE -
    // a per-hash spread here is O(n^2) and froze the UI with a large cache.
    const hydrateRenderIndex = async () => {
        const cached = await readIndexMap();
        const next = { ...renderState.value };
        for (const [hash, videoId] of Object.entries(cached)) next[hash] = completedEntry(videoId);
        renderState.value = next;
    };

    // Bulk delta reconcile. Throttled (cooldown) so rapid tab toggling can't
    // spam it; force=true (initial load) bypasses the cooldown.
    const syncRenderIndex = async (force = false) => {
        if (!config.hasToken || indexSyncing) return;
        if (!force && Date.now() - lastIndexSyncMs < SYNC_COOLDOWN_MS) return;
        indexSyncing = true;
        try {
            const since = (await indexStore.get<number>(CURSOR_FIELD)) ?? 0;
            const res = await tauri.renderedIndex(since);
            const m = await readIndexMap();
            const next = { ...renderState.value };
            for (const [hash, videoId] of Object.entries(res.map || {})) {
                m[hash] = videoId;
                next[hash] = completedEntry(videoId);
            }
            for (const hash of res.removed || []) {
                delete m[hash];
                if (next[hash]?.status === 'completed') delete next[hash];
            }
            renderState.value = next; // single reactive assignment for the whole delta
            await writeIndexMap(m);
            if (res.synced_at) { await indexStore.set(CURSOR_FIELD, res.synced_at); await indexStore.save(); }
            lastIndexSyncMs = Date.now();
        } catch {
            // ignore - real errors surface on render click; next sync retries
        } finally {
            indexSyncing = false;
        }
    };

    // Live poll for the few renders that are mid-flight (queued this
    // session). Completed/none are left to the bulk index. Only runs while
    // the Demos tab is the active, visible view.
    let inProgressTimer: number | undefined;
    const pollInProgress = async () => {
        if (!config.hasToken || document.hidden || !active) return;
        const hashes = Object.entries(renderState.value)
            .filter(([, s]) => s.status === 'pending' || s.status === 'rendering' || s.status === 'uploading')
            .map(([h]) => h);
        for (const h of hashes) {
            try {
                const s = await tauri.getRenderStatus(h);
                renderState.value = { ...renderState.value, [h]: s };
                if (s.status === 'completed' && s.youtube_video_id) {
                    const m = await readIndexMap(); m[h] = s.youtube_video_id; await writeIndexMap(m);
                }
            } catch { /* ignore */ }
        }
    };

    const onDemosVisibility = () => {
        if (!document.hidden && active) void syncRenderIndex();
    };

    // Demos with a completed YouTube render (drives the "has video" sort/filter).
    const hasYoutube = (d: DemoLibraryEntry): boolean =>
        !!d.hash && renderState.value[d.hash]?.status === 'completed';

    // -- search / sort / filter ---------------------------------------
    type SortKey = 'date_desc' | 'date_asc' | 'name_asc' | 'name_desc' | 'render_first';
    const sortKey = ref<SortKey>('date_desc');
    const search = ref('');

    type RowFilter = 'all' | 'in_progress' | 'uploaded' | 'not_uploaded' | 'error' | 'rendered';
    const rowFilter = ref<RowFilter>('all');

    type StatusKind = 'inprogress' | 'done' | 'duplicate' | 'error' | 'none' | 'held' | 'comps';
    interface RowStatus { label: string; color: string; kind: StatusKind; hint: string }

    // Resolve the status shown on a row: the live queue wins (it's
    // happening right now) and otherwise we fall back to the persisted
    // cache status from the disk listing.
    const resolveStatus = (d: DemoLibraryEntry): RowStatus => {
        const DUP_HINT = 'This exact run is already on defrag.racing - nothing to upload.';
        const DONE_HINT = 'Safely backed up to your defrag.racing profile.';
        const HELD_HINT = 'This looks like a run of a map being played in comps this week, so it was NOT backed up - '
            + 'a comps run published mid-round cannot be taken back. Choose what happens to it below.';
        const COMPS_HINT = 'Entered into this week\'s comps round. It is on defrag.racing but stays private until the round ends.';
        const live = queueByPath.value.get(d.path);
        if (live) {
            switch (live.status) {
                case 'pending':   return { label: 'Waiting',           color: 'text-neutral-400', kind: 'inprogress', hint: 'Waiting its turn in the backup queue.' };
                // "Hashing" is jargon; show "Checking" and explain in the tooltip.
                case 'hashing':   return { label: 'Checking…',         color: 'text-brand-400',   kind: 'inprogress', hint: 'Making a short fingerprint of the file to check whether this exact run is already on defrag.racing - so the same demo is never uploaded twice.' };
                case 'uploading': return { label: 'Uploading…',        color: 'text-brand-400',   kind: 'inprogress', hint: 'Sending the demo to your defrag.racing profile.' };
                case 'done':      return { label: 'Uploaded',          color: 'text-emerald-400', kind: 'done',       hint: DONE_HINT };
                case 'duplicate': return { label: 'Already backed up', color: 'text-cyan-400',    kind: 'duplicate',  hint: DUP_HINT };
                case 'error':     return { label: 'Error',             color: 'text-red-400',     kind: 'error',      hint: 'Backup failed - click Retry to try again.' };
                case 'held_for_comps': return { label: 'Held for comps', color: 'text-amber-300', kind: 'held',  hint: HELD_HINT };
                case 'comps_entered':  return { label: 'Entered in comps', color: 'text-amber-300/80', kind: 'comps', hint: COMPS_HINT };
            }
        }
        if (d.upload_status === 'done')      return { label: 'Backed up',         color: 'text-emerald-400/80', kind: 'done',      hint: DONE_HINT };
        if (d.upload_status === 'duplicate') return { label: 'Already backed up', color: 'text-cyan-400/80',    kind: 'duplicate', hint: DUP_HINT };
        if (d.upload_status === 'comps')     return { label: 'Entered in comps',  color: 'text-amber-300/80',   kind: 'comps',     hint: COMPS_HINT };
        if (d.upload_status === 'held_for_comps') return { label: 'Held for comps', color: 'text-amber-300',    kind: 'held',      hint: HELD_HINT };
        return { label: 'Not uploaded', color: 'text-neutral-500', kind: 'none', hint: 'Not backed up yet. Turn on auto-backup (top of this tab) and it\'ll be uploaded automatically.' };
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
                    case 'rendered':     return hasYoutube(d);
                }
            });
        }
        result.sort((a, b) => {
            switch (sortKey.value) {
                case 'date_desc': return b.mtime - a.mtime;
                case 'date_asc':  return a.mtime - b.mtime;
                case 'name_asc':  return a.filename.localeCompare(b.filename);
                case 'name_desc': return b.filename.localeCompare(a.filename);
                case 'render_first': {
                    // Demos with a YouTube video first, newest within each group.
                    const av = hasYoutube(a) ? 0 : 1;
                    const bv = hasYoutube(b) ? 0 : 1;
                    return av !== bv ? av - bv : b.mtime - a.mtime;
                }
            }
        });
        // While picking the second demo to compare, float same-map demos to the
        // top so the obvious choices are right there (stable - keeps the sort
        // above within each group).
        if (compareSel.value.length && compareMapKey.value) {
            result.sort((a, b) => Number(isSameMapAsA(b)) - Number(isSameMapAsA(a)));
        }
        return result;
    });

    // -- list virtualisation ------------------------------------------
    // The library can be many thousands of rows. Rendering them all puts
    // that many <li> in the DOM and makes every reactive update (a status
    // change, a re-sort) walk the whole tree. We window it: only the rows
    // intersecting the viewport (plus a small overscan) are rendered, held
    // in place by a fixed row height and a spacer the full list's tall.
    // ROW_H must match the rendered row height exactly or scrolling drifts.
    const ROW_H = 53;
    const OVERSCAN = 8;
    const scrollEl = ref<HTMLElement | null>(null);
    const scrollTop = ref(0);
    const viewportH = ref(0);
    const onListScroll = () => {
        const el = scrollEl.value;
        if (!el) return;
        scrollTop.value = el.scrollTop;
        viewportH.value = el.clientHeight;
    };
    const measureViewport = () => {
        const el = scrollEl.value;
        if (el) viewportH.value = el.clientHeight;
    };
    const startIndex = computed(() =>
        Math.max(0, Math.floor(scrollTop.value / ROW_H) - OVERSCAN),
    );
    const endIndex = computed(() => {
        const visibleCount = Math.ceil((viewportH.value || 600) / ROW_H) + OVERSCAN * 2;
        return Math.min(filteredDemos.value.length, startIndex.value + visibleCount);
    });
    // Each rendered row carries its absolute offset so it sits at the right
    // place inside the full-height spacer.
    const visibleDemos = computed(() =>
        filteredDemos.value.slice(startIndex.value, endIndex.value).map((d, i) => ({
            d,
            top: (startIndex.value + i) * ROW_H,
        })),
    );

    // -- live backup progress -----------------------------------------
    // The session summary counts terminal results, but while a big (or
    // CPU-throttled) hash is in flight nothing changes for seconds and the
    // UI looks frozen. This drives a live strip with the current file +
    // a spinner so "working slowly" reads differently from "stuck".
    const backupCounts = computed(() => {
        let hashing = 0, uploading = 0, pending = 0;
        for (const it of queue.value.items) {
            if (it.status === 'hashing') hashing++;
            else if (it.status === 'uploading') uploading++;
            else if (it.status === 'pending') pending++;
        }
        return { hashing, uploading, pending, remaining: hashing + uploading + pending };
    });
    // Only call it "backing up" when the watcher is actually running and not
    // paused - otherwise a leftover Pending row would show a misleading
    // "Backing up 0/1" with nothing able to move it.
    const backupActive = computed(
        () => config.autoUploadRunning && !paused.value && backupCounts.value.remaining > 0,
    );
    // Moving denominator: done-this-session + whatever is still queued.
    const backupTotal = computed(() => queue.value.processed_count + backupCounts.value.remaining);
    const backupPct = computed(() => {
        const total = backupTotal.value;
        return total > 0 ? Math.round((queue.value.processed_count / total) * 100) : 0;
    });
    const backupCurrent = computed(() =>
        queue.value.items.find((i) => i.status === 'hashing' || i.status === 'uploading') ?? null,
    );
    const backupCurrentLabel = computed(() => {
        const c = backupCurrent.value;
        if (!c) return backupCounts.value.pending > 0 ? 'Queued…' : '';
        const verb = c.status === 'uploading' ? 'Uploading' : 'Checking';
        const bps = c.status === 'uploading' ? c.upload_throughput_bps : c.hash_throughput_bps;
        const speed = bps && bps > 0 ? ` · ${(bps / 1_000_000).toFixed(1)} MB/s` : '';
        return `${verb} ${c.filename}${speed}`;
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
    let unlistenOpenDemo: UnlistenFn | null = null;

    // ---- opening .dm_68 files ---------------------------------------
    // The right-click entry is registered by the installer and re-asserted on
    // every start; becoming the DEFAULT program is the user's call, asked once
    // and then only in Settings. Most people already have DemoCleaner3 on this
    // file type and nothing here takes it away from them.
    const assoc = ref<DemoAssocStatus | null>(null);
    const assocBusy = ref(false);
    const assocNote = ref<string | null>(null);

    const refreshAssoc = async () => {
        try { assoc.value = await tauri.demoAssocStatus(); } catch { /* the card just stays away */ }
    };

    const showAssocOffer = computed(() =>
        !!assoc.value?.supported && !assoc.value.is_default && !config.config.demo_assoc_asked,
    );

    const answerAssoc = async (makeDefault: boolean) => {
        assocBusy.value = true;
        assocNote.value = null;
        try {
            if (makeDefault) {
                assoc.value = await tauri.demoAssocMakeDefault();

                // Windows keeps its own UserChoice once somebody has picked a
                // program, and an app may not write it. Say so plainly instead
                // of leaving a button that looks like it did nothing.
                if (!assoc.value.is_default) {
                    assocNote.value =
                        'Windows keeps its own choice for this file type. Right-click a .dm_68, choose "Open with" → "Choose another app", pick Defrag Launcher and tick "Always".';
                }
            }

            await config.save({ demo_assoc_asked: true });
        } catch (e: any) {
            assocNote.value = e?.toString?.() ?? 'Could not change the file association';
        } finally {
            assocBusy.value = false;
        }
    };

    let unlistenMapProgress: UnlistenFn | null = null;
    const refreshPaused = async () => {
        try { paused.value = await tauri.isAutoUploadPaused(); } catch { paused.value = false; }
    };

    // Coalesce upload_state_changed bursts. The backend emits up to 20x/sec
    // during a rescan; assigning queue.value each time would re-run the
    // allRows + filteredDemos (sort over the whole library) + re-render on
    // every emit. Stash the latest snapshot and apply at most once per
    // animation frame, so a 20-emit burst costs one rebuild, not twenty.
    let pendingSnapshot: UploadStateSnapshot | null = null;
    let applyFrame = 0;
    const applyPendingSnapshot = () => {
        applyFrame = 0;
        if (!pendingSnapshot) return;
        const snap = pendingSnapshot;
        pendingSnapshot = null;
        queue.value = snap;
        if (snap.processed_count > lastTerminalCount) {
            lastTerminalCount = snap.processed_count;
            scheduleRelist();
        }
    };

    onMounted(async () => {
        queue.value = await tauri.getUploadState();
        lastTerminalCount = queue.value.processed_count;
        await refreshPaused();
        await refreshThrottle();
        await pollRateLimit();
        await refreshList();

        // Paint completed renders from the persisted store instantly, then
        // reconcile via a bulk delta (replaces the old 100-row per-demo warmup).
        await hydrateRenderIndex();
        void syncRenderIndex(true);

        rateLimitPollTimer = window.setInterval(pollRateLimit, 1000);
        nowTickTimer = window.setInterval(() => { nowMs.value = Date.now(); }, 250);
        inProgressTimer = window.setInterval(() => { void pollInProgress(); }, 10_000);
        document.addEventListener('visibilitychange', onDemosVisibility);
        measureViewport();
        window.addEventListener('resize', measureViewport);

        // Cold start: the OS handed us a demo before the window existed, so
        // it is waiting in the backend rather than in an event nobody heard.
        const pendingDemo = await tauri.takePendingOpenDemo();
        if (pendingDemo) void openExternalDemo(pendingDemo);

        unlistenOpenDemo = await listen<string>('open-demo', (ev) => {
            void openExternalDemo(ev.payload);
        });

        void refreshAssoc();

        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            // Keep only the freshest snapshot; apply on the next frame so a
            // burst collapses into a single rebuild + re-list.
            pendingSnapshot = ev.payload;
            if (!applyFrame) applyFrame = requestAnimationFrame(applyPendingSnapshot);
        });

        unlistenMapProgress = await listen<{ map: string; phase: string; received: number; total: number | null }>(
            'demo-map-progress',
            (ev) => {
                if (!preparingMap.value) return;
                mapProgress.value = ev.payload;
                // The name we started with came from the filename; the backend
                // reads the real one out of the demo and reports that. Show
                // what is actually being fetched.
                if (ev.payload.map) preparingMap.value = ev.payload.map;
            },
        );
    });

    // KeepAlive caches this view across tab switches, so onMounted runs
    // only once. Re-list on re-entry to pick up demos recorded / deleted
    // while the user was on another tab (the event-driven relist only
    // covers watcher activity). Skip the first activation - onMounted
    // already listed - so we don't double-list on initial load.
    let activatedOnce = false;
    onActivated(() => {
        active = true;
        measureViewport();
        // Re-entering Demos: reconcile render videos (throttled) so a video
        // that completed while you were elsewhere shows up without a restart.
        void syncRenderIndex();
        if (!activatedOnce) { activatedOnce = true; return; }
        void refreshList();
    });

    onDeactivated(() => {
        active = false;
        // Leaving the Demos tab while a demo plays: end playback and clear the
        // overlay, so the engine stops and coming back shows just the demo list
        // (not a stale player). Clearing the target unmounts the panel, which
        // stops the engine(s).
        playerTarget.value = null;
        compareTarget.value = null;
        compareSel.value = [];
    });

    onUnmounted(() => {
        if (unlisten) unlisten();
        if (unlistenOpenDemo) unlistenOpenDemo();
        if (unlistenMapProgress) unlistenMapProgress();
        if (applyFrame) cancelAnimationFrame(applyFrame);
        if (rateLimitPollTimer !== undefined) window.clearInterval(rateLimitPollTimer);
        if (nowTickTimer !== undefined) window.clearInterval(nowTickTimer);
        if (inProgressTimer !== undefined) window.clearInterval(inProgressTimer);
        if (relistTimer !== undefined) window.clearTimeout(relistTimer);
        document.removeEventListener('visibilitychange', onDemosVisibility);
        window.removeEventListener('resize', measureViewport);
    });

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
        if (map) openExternal(`https://defrag.racing/maps/${encodeURIComponent(map)}`).catch(() => {});
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
        if (!d.hash) return 'Render to YouTube';
        if (rendering.value.has(d.hash)) return 'Working…';
        const st = renderState.value[d.hash];
        if (!st || !st.has_render) return 'Render to YouTube';
        switch (st.status) {
            case 'completed': return 'Watch on YouTube';
            case 'pending':   return 'Queued';
            case 'rendering': return 'Rendering…';
            case 'uploading': return 'Uploading…';
            case 'failed':    return 'Retry render';
            default:          return 'Queued';
        }
    };
    const renderIsCompleted = (d: DemoLibraryEntry): boolean =>
        !!d.hash && renderState.value[d.hash]?.status === 'completed';

    const renderClickFor = (d: DemoLibraryEntry) => {
        if (!d.hash) return; // render button is disabled without a hash anyway
        const st = renderState.value[d.hash];
        if (st?.status === 'completed' && st.youtube_url) {
            return openExternal(st.youtube_url).catch(() => {});
        }
        // pending / rendering / uploading: clicking again is a no-op.
        if (st && (st.status === 'pending' || st.status === 'rendering' || st.status === 'uploading')) return;
        // No render yet (or a failed one): confirm first. The launcher hits
        // the same paid render farm as the website, so we gate every render
        // behind the same cost + etiquette acknowledgement the web shows.
        askRender(d);
    };

    // -- Render confirmation modal ------------------------------------
    const showRenderConfirm = ref(false);
    const renderTarget = ref<DemoLibraryEntry | null>(null);
    const etiquetteAccepted = ref(false);
    const askRender = (d: DemoLibraryEntry) => {
        renderTarget.value = d;
        etiquetteAccepted.value = false;
        showRenderConfirm.value = true;
    };
    const cancelRenderConfirm = () => {
        showRenderConfirm.value = false;
        renderTarget.value = null;
    };
    const confirmRender = () => {
        const d = renderTarget.value;
        showRenderConfirm.value = false;
        renderTarget.value = null;
        if (d) void renderDemo(d);
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

    // Answering a demo the comps guard is holding. Both answers go through
    // the same worker the watcher uses, so the row's status updates through
    // the normal upload_state_changed stream rather than being faked here.
    const compsBusy = ref<Set<string>>(new Set());
    const answerComps = async (path: string, enter: boolean) => {
        if (compsBusy.value.has(path)) return;
        compsBusy.value = new Set(compsBusy.value).add(path);
        try {
            if (enter) await tauri.compsEnter(path);
            else await tauri.compsUploadNormally(path);
            void tauri.compsMarkIntroSeen();
        } catch (e: any) {
            toggleError.value = e?.toString?.() ?? 'Could not send the demo';
        } finally {
            const next = new Set(compsBusy.value);
            next.delete(path);
            compsBusy.value = next;
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
    <div class="flex-1 flex flex-col min-h-0 relative">
        <!-- Embedded player overlay: covers the whole Demos section while a
             demo plays; the panel's ✕ closes it (stops + clears). Same overlay
             hosts the two-engine comparison when compareTarget is set. -->
        <div v-if="playerTarget || compareTarget" class="absolute inset-0 z-30 bg-neutral-950">
            <DemoPlayerPanel
                :demo="playerTarget"
                :compare="compareTarget"
                @close="playerTarget = null; compareTarget = null"
            />
        </div>

        <!-- Preparing the map (check / download) before the player opens. -->
        <div
            v-if="preparingMap"
            class="absolute inset-0 z-40 bg-neutral-950/80 flex flex-col items-center justify-center gap-3 px-8"
        >
            <svg class="w-7 h-7 animate-spin text-emerald-400" viewBox="0 0 24 24" fill="none">
                <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
            </svg>
            <div class="text-sm text-neutral-300">
                Preparing map <span class="font-semibold text-white">{{ preparingMap }}</span>…
            </div>

            <!-- Checking installed maps (cold scan of a big collection is slow). -->
            <div v-if="!mapProgress || mapProgress.phase === 'checking'" class="text-xs text-neutral-500">
                Checking your installed maps…
            </div>

            <!-- Downloading: real progress bar (percent when the size is known,
                 otherwise just the downloaded amount). -->
            <template v-else>
                <div class="w-72 max-w-full h-2 rounded-full bg-white/10 overflow-hidden">
                    <div
                        class="h-full bg-emerald-400 transition-[width] duration-150"
                        :style="{ width: mapPercent !== null ? mapPercent + '%' : '100%' }"
                        :class="mapPercent === null ? 'animate-pulse' : ''"
                    ></div>
                </div>
                <div class="text-xs text-neutral-400 tabular-nums">
                    <template v-if="mapPercent !== null">
                        Downloading… {{ mapPercent }}%
                        <span class="text-neutral-600">
                            ({{ fmtMB(mapProgress.received) }} / {{ fmtMB(mapProgress.total ?? 0) }})
                        </span>
                    </template>
                    <template v-else>
                        Downloading… {{ fmtMB(mapProgress.received) }}
                    </template>
                </div>
            </template>
        </div>

        <!-- Map download failed: tell the user how to recover. -->
        <div
            v-if="mapError"
            class="absolute inset-0 z-40 bg-neutral-950/80 flex items-center justify-center p-6"
            @click.self="mapError = null"
        >
            <div class="max-w-md w-full rounded-lg border border-red-500/30 bg-neutral-900 p-5 shadow-xl">
                <div class="flex items-start gap-3">
                    <svg class="w-6 h-6 text-red-400 flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v4m0 4h.01M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                    </svg>
                    <div class="min-w-0">
                        <h3 class="font-semibold text-white">Couldn't download the map</h3>
                        <p class="text-sm text-neutral-300 mt-1">
                            We couldn't fetch
                            <span class="font-semibold text-white">{{ mapError.map }}</span>
                            automatically, so the demo can't play yet. Install the map
                            manually (Maps tab or defrag.racing), then try again. If it
                            still fails, the map may be missing from the server - please
                            contact an admin.
                        </p>
                        <p class="text-xs text-neutral-500 mt-2 break-words">{{ mapError.detail }}</p>
                    </div>
                </div>
                <div class="mt-4 flex justify-end">
                    <button
                        class="px-3 py-1.5 rounded bg-neutral-700 hover:bg-neutral-600 text-sm"
                        @click="mapError = null"
                    >
                        Close
                    </button>
                </div>
            </div>
        </div>

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
                            Watching your demos folder. Every new run you record is copied to your <strong class="text-neutral-300">defrag.racing</strong> profile within ~30s - so you never lose a demo, even after a game crash. Each one is quickly checked first so the same run is never uploaded twice. New demos show up in the list below; render any to YouTube.
                        </template>
                        <template v-else-if="config.autoUploadRunning && paused">
                            Auto-backup is paused. The watcher still notices new demos, but nothing uploads until you click <strong class="text-brand-400">Resume</strong>.
                        </template>
                        <template v-else>
                            <strong class="text-neutral-300">Auto-backup</strong> keeps a safe online copy of every Defrag run you record: it watches this folder and uploads each new demo to your defrag.racing profile, so a crash or a wiped drive never loses your runs. Click <strong class="text-brand-400">Start</strong> to turn it on - your existing demos are listed below either way.
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
                <TokenFeatureList />
            </ul>
            <div class="font-semibold text-emerald-300 mb-1">Works without a token:</div>
            <ul class="space-y-0.5 pl-1 mb-2 text-emerald-200/90">
                <TokenFreeFeatures />
            </ul>
            <div class="flex items-center justify-end gap-2">
                <button
                    class="px-3 py-1 rounded bg-amber-500/30 hover:bg-amber-500/40 text-amber-100 font-semibold flex-shrink-0"
                    @click="router.push({ name: 'settings' })"
                >Add token →</button>
            </div>
        </div>

        <!-- Asked once: should a double-clicked demo open here?
             The right-click entry is already in place and took nothing away
             from anybody - this is only about the default program, which most
             people have already given to DemoCleaner3. So it is a question,
             asked one time, and never again from the installer. -->
        <div
            v-if="showAssocOffer"
            class="px-5 py-3 border-b border-white/10 bg-white/[0.03] text-xs text-neutral-300"
        >
            <div class="font-semibold text-neutral-200 mb-1">Open .dm_68 demos in the launcher?</div>
            <p class="text-neutral-400 mb-2">
                Right-clicking a demo already offers "Play in Defrag Launcher". This is about
                double-clicking one: it would open here instead of in whatever you use now.
                You can change it in Settings whenever you like.
            </p>
            <div class="flex items-center justify-end gap-2">
                <button
                    class="px-3 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300"
                    :disabled="assocBusy"
                    @click="answerAssoc(false)"
                >No thanks</button>
                <button
                    class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 font-semibold"
                    :disabled="assocBusy"
                    @click="answerAssoc(true)"
                >Yes, open them here</button>
            </div>
        </div>

        <p v-if="assocNote" class="px-5 py-2 border-b border-white/10 bg-white/[0.03] text-xs text-neutral-400">
            {{ assocNote }}
        </p>

        <!-- Update banner lives at App level now (shows on every tab),
             so it's no longer duplicated here. -->

        <!-- live backup progress -->
        <div
            v-if="backupActive"
            class="px-5 py-2 border-b border-white/[0.06] bg-brand-500/[0.06]"
        >
            <div class="flex items-center gap-2 text-xs">
                <svg class="w-3.5 h-3.5 animate-spin text-brand-400 flex-shrink-0" viewBox="0 0 24 24" fill="none">
                    <circle class="opacity-20" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                    <path class="opacity-90" d="M22 12a10 10 0 0 1-10 10" stroke="currentColor" stroke-width="4" stroke-linecap="round" />
                </svg>
                <span class="text-brand-200 font-semibold flex-shrink-0">
                    Backing up {{ queue.processed_count }}/{{ backupTotal }}
                </span>
                <span class="text-neutral-400 truncate min-w-0">{{ backupCurrentLabel }}</span>
                <span class="text-neutral-500 ml-auto flex-shrink-0 whitespace-nowrap">{{ backupCounts.remaining }} left</span>
            </div>
            <div class="mt-1.5 h-1 rounded-full bg-white/10 overflow-hidden">
                <div class="h-full bg-brand-500 transition-all duration-300" :style="{ width: backupPct + '%' }"></div>
            </div>
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
                        { v: 'rendered',     label: 'Rendered' },
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
                        { v: 'render_first', label: 'Has video' },
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

        <!-- Comparison pick banner: shown after Compare is clicked on a demo.
             Add up to 4 demos (same-map ones float to the top), then Start. -->
        <div
            v-if="compareSel.length"
            class="flex-shrink-0 flex items-center gap-2 px-5 py-2 bg-amber-500/10 border-b border-amber-500/30 text-sm"
        >
            <span class="text-amber-200 font-semibold flex-shrink-0">Compare ({{ compareSel.length }}/{{ MAX_COMPARE }}):</span>
            <span class="text-amber-300/70 flex-shrink-0 truncate">pick {{ compareSel.length < 2 ? 'at least one more' : 'up to ' + MAX_COMPARE }} demo, same map first</span>
            <div class="ml-auto flex items-center gap-2 flex-shrink-0">
                <button
                    class="px-3 py-1 rounded text-xs font-semibold disabled:opacity-40 disabled:cursor-not-allowed bg-purple-500/25 hover:bg-purple-500/35 text-purple-200"
                    :disabled="compareSel.length < 2"
                    @click="launchCompare"
                >Start compare ⚖ ({{ compareSel.length }})</button>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-xs"
                    @click="cancelComparePick"
                >Cancel</button>
            </div>
        </div>

        <!-- the list -->
        <div ref="scrollEl" class="flex-1 overflow-auto queue-scroll" @scroll="onListScroll">
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
            <ul v-else class="relative" :style="{ height: filteredDemos.length * ROW_H + 'px' }">
                <li
                    v-for="{ d, top } in visibleDemos"
                    :key="d.path"
                    class="absolute left-0 right-0 px-5 flex items-center gap-3 overflow-hidden border-b border-white/[0.04] hover:bg-white/[0.02]"
                    :class="{
                        'opacity-40': compareSel.length && compareIndexOf(d) < 0 && !isSameMapAsA(d),
                        'bg-amber-500/[0.06]': compareIndexOf(d) >= 0,
                    }"
                    :style="{ top: top + 'px', height: ROW_H + 'px' }"
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

                    <!-- live / persisted status (hover for a plain-language note) -->
                    <div class="text-xs font-semibold flex-shrink-0 cursor-help" :class="resolveStatus(d).color" :title="resolveStatus(d).hint">
                        {{ resolveStatus(d).label }}
                    </div>

                    <!-- A demo the comps guard is holding. The two buttons sit
                         on the row itself because that is where the user is
                         looking after a run; the Comps tab repeats them for
                         anyone who gets there first. -->
                    <template v-if="resolveStatus(d).kind === 'held'">
                        <button
                            class="text-[11px] px-2 py-0.5 rounded bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 font-semibold disabled:opacity-50 flex-shrink-0 whitespace-nowrap"
                            :disabled="compsBusy.has(d.path)"
                            :title="`Enter ${d.filename} into this week's comps round`"
                            @click.stop="answerComps(d.path, true)"
                        >Enter into comps</button>
                        <button
                            class="text-[11px] px-2 py-0.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50 flex-shrink-0 whitespace-nowrap"
                            :disabled="compsBusy.has(d.path)"
                            title="Back it up like any other demo. Decides this file only - the next run on a comps map is asked about again."
                            @click.stop="answerComps(d.path, false)"
                        >Upload normally</button>
                    </template>

                    <!-- retry when the live upload errored -->
                    <button
                        v-if="resolveStatus(d).kind === 'error' && config.autoUploadRunning"
                        class="text-[11px] px-2 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 font-semibold disabled:opacity-50 flex-shrink-0"
                        :disabled="retrying.has(d.path)"
                        :title="`Re-queue ${d.filename} for upload`"
                        @click.stop="retryUpload(d.path)"
                    >{{ retrying.has(d.path) ? 'Retrying…' : 'Retry' }}</button>

                    <!-- comparison pick mode: rows become add/remove selectors -->
                    <template v-if="compareSel.length">
                        <button
                            v-if="compareIndexOf(d) >= 0"
                            class="px-3 py-1 rounded text-xs font-semibold bg-amber-500/30 hover:bg-amber-500/40 text-amber-200 flex-shrink-0 whitespace-nowrap"
                            title="Remove from comparison"
                            @click.stop="toggleCompareSel(d)"
                        >✓ Demo {{ compareLetter(compareIndexOf(d)) }} (remove)</button>
                        <button
                            v-else
                            class="px-3 py-1 rounded text-xs font-semibold bg-amber-500/20 hover:bg-amber-500/30 text-amber-200 flex-shrink-0 whitespace-nowrap disabled:opacity-40 disabled:cursor-not-allowed"
                            :disabled="compareSel.length >= MAX_COMPARE"
                            :title="compareSel.length >= MAX_COMPARE ? 'Maximum of 4 demos' : (isSameMapAsA(d) ? 'Add this run to the comparison' : 'Different map - add anyway')"
                            @click.stop="toggleCompareSel(d)"
                        >+ Add ⚖</button>
                    </template>

                    <!-- normal mode actions (hidden while picking a comparison) -->
                    <template v-else>
                    <!-- play embedded (Windows + Linux) -->
                    <button
                        v-if="isEmbedSupported"
                        class="px-3 py-1 rounded text-xs font-semibold bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 flex-shrink-0 flex items-center gap-1 whitespace-nowrap"
                        title="Plays right here in the launcher - instant, no rendering or upload needed"
                        @click.stop="playDemo(d)"
                    >▶ Play instantly in launcher</button>

                    <!-- compare two demos side by side (premium, token-gated) -->
                    <button
                        v-if="isEmbedSupported && config.hasToken"
                        class="px-3 py-1 rounded text-xs font-semibold bg-purple-500/20 hover:bg-purple-500/30 text-purple-300 flex-shrink-0 flex items-center gap-1 whitespace-nowrap"
                        title="Compare this run side by side with another demo - two engines, locked together"
                        @click.stop="startComparePick(d)"
                    >⚖ Compare</button>

                    <!-- render / play YouTube -->
                    <button
                        v-if="config.hasToken"
                        class="px-3 py-1 rounded text-xs font-semibold disabled:opacity-50 flex-shrink-0 flex items-center gap-1 whitespace-nowrap"
                        :class="renderIsCompleted(d)
                            ? 'bg-red-500/20 hover:bg-red-500/30 text-red-300'
                            : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-300'"
                        :disabled="!d.hash || (!!d.hash && rendering.has(d.hash))"
                        :title="renderIsCompleted(d)
                            ? 'Open the rendered run on YouTube'
                            : (d.hash
                                ? 'Render this run to a video and upload it to the defrag.racing YouTube channel'
                                : ((d.upload_status === 'done' || d.upload_status === 'duplicate')
                                    ? 'Backed up, but its fingerprint is missing locally - turn on auto-backup (Start) to recompute it, then you can render'
                                    : 'Back this demo up first (turn on auto-backup), then you can render it'))"
                        @click="renderClickFor(d)"
                    >
                        <svg v-if="renderIsCompleted(d)" class="w-3.5 h-3.5 flex-shrink-0" viewBox="0 0 24 24" fill="currentColor"><path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814z"/><path d="M9.545 15.568V8.432L15.818 12l-6.273 3.568z" fill="#0a0a0a"/></svg>
                        <span>{{ renderLabelFor(d) }}</span>
                    </button>
                    </template>
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

        <!-- Render confirmation. Same cost + etiquette acknowledgement the
             website shows - the launcher queues onto the same paid render
             farm, so it gets the same gate. -->
        <template v-if="showRenderConfirm">
            <div class="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 p-4" @click.self="cancelRenderConfirm">
                <div class="bg-neutral-900 border border-white/10 rounded-xl p-5 shadow-2xl max-w-md w-full">
                    <div class="flex items-center gap-3 mb-3">
                        <div class="w-10 h-10 rounded-lg bg-red-500/20 flex items-center justify-center flex-shrink-0">
                            <svg class="w-5 h-5 text-red-400" viewBox="0 0 24 24" fill="currentColor"><path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814z"/><path d="M9.545 15.568V8.432L15.818 12l-6.273 3.568z" fill="#fff"/></svg>
                        </div>
                        <h3 class="text-sm font-bold text-neutral-100">Render to YouTube?</h3>
                    </div>
                    <p class="text-xs text-neutral-400 mb-3">This queues your demo to be rendered into a video and uploaded to the defrag.racing YouTube channel. Rendering can take several minutes.</p>

                    <div class="border border-red-500/30 bg-red-500/[0.06] rounded-lg p-3 mb-3">
                        <p class="text-[11px] text-neutral-300 leading-relaxed">
                            <span class="font-bold text-red-300">Renders cost real money.</span>
                            The render farm time and storage are paid for by defrag.racing out of the project's own pocket, and YouTube caps how many videos demome can upload per day. Every render you queue spends part of that shared, limited budget - so please only render runs that are worth keeping.
                        </p>
                    </div>

                    <div class="border border-white/10 bg-white/[0.02] rounded-lg p-3 mb-3">
                        <h4 class="text-xs font-bold text-neutral-100 mb-2">Render etiquette</h4>
                        <ul class="text-[11px] text-neutral-400 space-y-1.5 list-disc pl-4">
                            <li>Render your best run. Don't queue your whole time history when you already have - or will beat in minutes or hours - a faster time.</li>
                            <li>Several near-identical times on the same map, a few ms apart? Pick one.</li>
                            <li>Slower time but a genuinely cool trick or something worth showing off? That's totally fine, go for it.</li>
                        </ul>
                    </div>

                    <label class="flex items-start gap-2 mb-4 cursor-pointer select-none">
                        <input type="checkbox" v-model="etiquetteAccepted" class="mt-0.5 w-3.5 h-3.5 rounded border-white/20 bg-white/5 accent-red-600" />
                        <span class="text-xs text-neutral-300">I won't render my whole time history - just the runs actually worth keeping.</span>
                    </label>

                    <div class="flex gap-2 justify-end">
                        <button @click="cancelRenderConfirm" class="px-3 py-1.5 text-xs font-medium text-neutral-300 bg-white/5 hover:bg-white/10 border border-white/10 rounded-lg transition-colors">Cancel</button>
                        <button @click="confirmRender" :disabled="!etiquetteAccepted" class="px-3 py-1.5 text-xs font-bold text-white bg-red-600 hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg transition-colors">Render</button>
                    </div>
                </div>
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
