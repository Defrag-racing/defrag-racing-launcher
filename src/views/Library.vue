<script setup lang="ts">
    // Local demo library. Lists every .dm_* in the configured demos
    // folder with its server state (uploaded? rendered? not yet?),
    // a name filter, sort (date / name), and a per-row Render button
    // that queues a YouTube render through /api/launcher/render-video.
    //
    // Demos that haven't been uploaded yet show a "Render" button
    // that uploads first - the backend returns 404+needs_upload in
    // that case and we don't try to be clever about it; we just tell
    // the user to wait for the watcher to pick the file up. Once it's
    // in the cache (Done/Duplicate), Render queues server-side.

    import { computed, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type DemoLibraryEntry, type RenderStatusResponse } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const demos = ref<DemoLibraryEntry[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);

    type SortKey = 'date_desc' | 'date_asc' | 'name_asc' | 'name_desc';
    const sortKey = ref<SortKey>('date_desc');
    const search = ref('');

    // Per-demo render state cache keyed by file hash. Populated lazily
    // when the user clicks Render (immediate write) or when a status
    // poll lands. Not persisted - it's just UI feedback for the
    // current session; the authoritative state is the notifications
    // feed on the server side.
    const renderState = ref<Record<string, RenderStatusResponse>>({});
    const rendering = ref<Set<string>>(new Set());

    const refresh = async () => {
        loading.value = true;
        error.value = null;
        try {
            demos.value = await tauri.listDemos();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to list demos';
        } finally {
            loading.value = false;
        }
    };

    onMounted(async () => {
        await refresh();
        // Best-effort render-status warmup for the visible page: for
        // each demo that already has a hash + upload_status, fire a
        // status check so existing renders show their YouTube URL
        // without the user clicking Render first. Stagger the
        // requests so the launcher-browse bucket doesn't see 5000
        // hits in 5 seconds if the user has a huge folder.
        if (config.hasToken) {
            await warmupRenderStates();
        }
    });

    let warmupCancelled = false;
    onUnmounted(() => { warmupCancelled = true; });

    const warmupRenderStates = async () => {
        // Only check the first 100 rows on initial mount; the rest
        // will be loaded on-demand when the user scrolls / clicks.
        const sample = demos.value
            .filter((d) => d.hash && d.upload_status)
            .slice(0, 100);
        for (const d of sample) {
            if (warmupCancelled) return;
            try {
                const s = await tauri.getRenderStatus(d.hash!);
                renderState.value = { ...renderState.value, [d.hash!]: s };
            } catch {
                // ignore - we'll surface real errors on click
            }
        }
    };

    const filteredDemos = computed(() => {
        const q = search.value.trim().toLowerCase();
        let result = demos.value.slice();
        if (q) {
            result = result.filter((d) => d.filename.toLowerCase().includes(q));
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

    const formatSize = (n: number): string => {
        if (n < 1024) return `${n} B`;
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
        return `${(n / 1024 / 1024).toFixed(1)} MB`;
    };

    const formatDate = (epochSec: number): string => {
        if (!epochSec) return '';
        return new Date(epochSec * 1000).toLocaleString();
    };

    // Demo filenames look like "mapname[mdf.physics]MM.SS.mmm(player).dm_68"
    // - pull out the map for quick display next to the filename. Best-
    // effort, falls back to the whole filename when the pattern
    // doesn't match.
    const mapFromFilename = (f: string): string | null => {
        const m = f.match(/^([^\[]+)\[/);
        return m ? m[1] : null;
    };

    const openMap = (mapname: string) => {
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => { /* best effort */ });
    };

    const renderDemo = async (d: DemoLibraryEntry) => {
        if (!d.hash) {
            error.value = `${d.filename} hasn't been uploaded yet - wait for the watcher to pick it up.`;
            return;
        }
        rendering.value.add(d.hash);
        rendering.value = new Set(rendering.value);
        error.value = null;
        try {
            const r = await tauri.requestRender(d.hash);
            switch (r._http_status) {
                case 200:
                    // Queued or already_queued. Sync render state cache.
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
                case 404:
                    error.value = 'Demo not yet uploaded to the server. The watcher will pick it up shortly - try again in a moment.';
                    break;
                case 429:
                    error.value = `Daily render quota reached (${r.error ?? '20/day'}).`;
                    break;
                case 403:
                    error.value = r.error ?? 'Your account is restricted from rendering.';
                    break;
                default:
                    error.value = r.error ?? `Unexpected response (HTTP ${r._http_status}).`;
            }
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Render request failed';
        } finally {
            rendering.value.delete(d.hash);
            rendering.value = new Set(rendering.value);
        }
    };

    const renderLabelFor = (d: DemoLibraryEntry): string => {
        if (!d.hash) return 'Render';
        if (d.hash && rendering.value.has(d.hash)) return 'Working…';
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

    const renderClickFor = (d: DemoLibraryEntry) => {
        if (!d.hash) return renderDemo(d);
        const st = renderState.value[d.hash];
        if (st?.status === 'completed' && st.youtube_url) {
            return openUrl(st.youtube_url).catch(() => {});
        }
        // pending / rendering / uploading: clicking again does nothing
        // (server short-circuits to already_queued anyway); failed:
        // server doesn't allow user re-queue so we leave the button
        // as a no-op signal that something needs admin attention.
        if (st && (st.status === 'pending' || st.status === 'rendering' || st.status === 'uploading')) {
            return;
        }
        return renderDemo(d);
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Library</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    All <code class="bg-black/40 px-1 rounded">.dm_*</code> files in
                    <code class="bg-black/40 px-1 rounded">{{ config.config.demos_path || '(folder not set)' }}</code>.
                    Render a demo to YouTube straight from here.
                </div>
            </div>
            <button
                class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-xs disabled:opacity-50 flex-shrink-0"
                :disabled="loading"
                @click="refresh"
            >{{ loading ? 'Loading…' : 'Refresh' }}</button>
        </header>

        <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 text-xs flex-wrap">
            <input
                v-model="search"
                type="text"
                placeholder="Search filename…"
                class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
            />
            <span class="text-neutral-500 mr-1">Sort:</span>
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
            <span class="text-neutral-500 ml-auto">{{ filteredDemos.length }} / {{ demos.length }}</span>
        </div>

        <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ error }}
        </p>

        <div class="flex-1 overflow-auto">
            <div v-if="loading && !demos.length" class="p-8 text-center text-sm text-neutral-500">
                Listing demos…
            </div>
            <div v-else-if="!demos.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">📁</div>
                    <div class="text-neutral-300 font-semibold">No demos found</div>
                    <p class="text-sm text-neutral-500">
                        Set or check your demos folder in Settings, then click Refresh.
                    </p>
                </div>
            </div>
            <div v-else-if="!filteredDemos.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-neutral-300 font-semibold">No demos match the filter</div>
                    <p class="text-sm text-neutral-500">
                        <button class="text-brand-400 hover:underline" @click="search = ''">Clear search</button>.
                    </p>
                </div>
            </div>
            <ul v-else class="divide-y divide-white/[0.04]">
                <li v-for="d in filteredDemos" :key="d.path" class="px-5 py-2 flex items-center gap-3">
                    <div class="flex-1 min-w-0">
                        <div class="text-sm text-neutral-200 truncate font-medium" :title="d.filename">
                            {{ d.filename }}
                        </div>
                        <div class="text-xs text-neutral-500 truncate flex items-center gap-2 mt-0.5">
                            <button
                                v-if="mapFromFilename(d.filename)"
                                class="text-brand-400 hover:underline"
                                @click="openMap(mapFromFilename(d.filename)!)"
                            >{{ mapFromFilename(d.filename) }}</button>
                            <span class="text-neutral-600" v-if="mapFromFilename(d.filename)">·</span>
                            <span>{{ formatSize(d.size_bytes) }}</span>
                            <span class="text-neutral-600">·</span>
                            <span>{{ formatDate(d.mtime) }}</span>
                            <span
                                v-if="d.upload_status"
                                class="ml-1 uppercase text-[10px] px-1 py-0.5 rounded bg-cyan-500/10 text-cyan-300"
                                :title="d.upload_status === 'done' ? 'Newly uploaded by the launcher' : 'Server already had this hash'"
                            >{{ d.upload_status }}</span>
                            <span
                                v-else
                                class="ml-1 uppercase text-[10px] px-1 py-0.5 rounded bg-white/5 text-neutral-400"
                                title="Not yet processed by the watcher"
                            >not uploaded</span>
                        </div>
                    </div>
                    <button
                        v-if="config.hasToken"
                        class="px-3 py-1 rounded text-xs font-semibold disabled:opacity-50 flex-shrink-0"
                        :class="(d.hash && renderState[d.hash]?.status === 'completed')
                            ? 'bg-red-500/20 hover:bg-red-500/30 text-red-300'
                            : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-300'"
                        :disabled="!d.hash || (!!d.hash && rendering.has(d.hash))"
                        :title="!d.hash ? 'Wait for the watcher to upload this demo first' : 'Queue a YouTube render'"
                        @click="renderClickFor(d)"
                    >{{ renderLabelFor(d) }}</button>
                </li>
            </ul>
        </div>
    </div>
</template>
