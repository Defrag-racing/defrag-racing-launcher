<script setup lang="ts">
    // Offline Maps sub-tab: maps installed locally in the engine's baseq3
    // folder. Map names come from each pk3's maps/*.bsp entries; thumbnails
    // are the pk3 levelshots, extracted lazily (IntersectionObserver) and
    // cached by the backend. No token needed - this is purely local. "Run
    // offline" launches straight into the map (already installed, so no
    // download).
    //
    // Paginated: the backend scans baseq3 once (cached to a manifest) and
    // returns only a page at a time, so we never load thumbnails for the
    // whole library - that pinned the disk at 100% on big libraries.

    import { onActivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { tauri, type OfflineMap, type OfflineMapPage } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const config = useConfigStore();

    const page = ref(1);
    const data = ref<OfflineMapPage | null>(null);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const search = ref('');
    const thumbs = ref<Record<string, string>>({});
    const requested = new Set<string>();
    const runningKey = ref<string | null>(null);

    const keyOf = (m: OfflineMap) => `${m.pk3_path}|${m.name}`;

    const load = async () => {
        if (!config.config.engine_path) {
            data.value = null;
            return;
        }
        loading.value = true;
        error.value = null;
        try {
            data.value = await tauri.listOfflineMaps(page.value, 24, search.value);
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to read local maps';
            data.value = null;
        } finally {
            loading.value = false;
        }
    };

    // Debounce search; reset to page 1 on a new query.
    let searchTimer: number | undefined;
    watch(search, () => {
        if (searchTimer !== undefined) window.clearTimeout(searchTimer);
        searchTimer = window.setTimeout(() => {
            page.value = 1;
            void load();
        }, 300);
    });

    const setPage = (p: number) => {
        const last = data.value?.last_page ?? 1;
        if (p < 1 || p > last || p === page.value) return;
        page.value = p;
        void load();
    };

    // Lazy thumbnail loading: extract a levelshot only once its card scrolls
    // near the viewport. With pagination this is bounded to one page worth.
    let observer: IntersectionObserver | null = null;
    const elMap = new WeakMap<Element, OfflineMap>();
    const registerCard = (el: Element | null, m: OfflineMap) => {
        if (!el || !observer) return;
        elMap.set(el, m);
        observer.observe(el);
    };
    const loadThumb = async (m: OfflineMap) => {
        const k = keyOf(m);
        if (requested.has(k) || !m.has_levelshot) return;
        requested.add(k);
        try {
            const url = await tauri.offlineMapThumb(m.pk3_path, m.name);
            if (url) thumbs.value = { ...thumbs.value, [k]: url };
        } catch { /* leave placeholder */ }
    };

    onMounted(() => {
        observer = new IntersectionObserver((entries) => {
            for (const e of entries) {
                if (e.isIntersecting) {
                    const m = elMap.get(e.target);
                    observer?.unobserve(e.target);
                    if (m) void loadThumb(m);
                }
            }
        }, { rootMargin: '300px' });
        void load();
    });
    onActivated(() => { void load(); });
    onUnmounted(() => {
        observer?.disconnect();
        observer = null;
        if (searchTimer !== undefined) window.clearTimeout(searchTimer);
    });

    const runOffline = async (m: OfflineMap, physics: 'vq3' | 'cpm') => {
        const k = `${keyOf(m)}:${physics}`;
        if (runningKey.value) return;
        runningKey.value = k;
        error.value = null;
        try {
            // Already in baseq3 -> launch directly, no download.
            await tauri.launchEngineArgs(`+${physics} "${m.name}"`);
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to run the map';
        } finally {
            runningKey.value = null;
        }
    };
    const isRunning = (m: OfflineMap, p: string) => runningKey.value === `${keyOf(m)}:${p}`;
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 text-xs">
            <input
                v-model="search"
                type="text"
                placeholder="Search local map name…"
                class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
            />
            <div v-if="data" class="text-neutral-500 whitespace-nowrap">
                page {{ data.current_page }} / {{ data.last_page }} · {{ data.total }} local
            </div>
            <button
                class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                :disabled="loading"
                @click="load"
            >{{ loading ? 'Scanning…' : 'Rescan' }}</button>
        </div>

        <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ error }}
        </p>

        <div class="flex-1 overflow-auto">
            <div v-if="loading && !data" class="p-8 text-center text-sm text-neutral-500">
                Scanning baseq3…
            </div>
            <div v-else-if="!config.config.engine_path" class="p-8 text-center text-sm text-neutral-500">
                Pick an engine in Settings to see your installed maps.
            </div>
            <div v-else-if="data && !data.total" class="p-8 text-center text-sm text-neutral-500">
                No maps found in <code class="bg-black/40 px-1 rounded">baseq3</code>. Install maps (or use
                "Run offline" on the online Maps tab) and they'll show up here.
            </div>
            <div v-else-if="data && !data.data.length" class="p-8 text-center text-sm text-neutral-500">
                No local maps match this search.
            </div>
            <ul v-else-if="data" class="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 gap-3 p-3">
                <li
                    v-for="m in data.data"
                    :key="keyOf(m)"
                    :ref="(el) => registerCard(el as Element | null, m)"
                    class="bg-neutral-900/40 border border-white/10 rounded-lg overflow-hidden flex flex-col hover:border-brand-500/40 transition-colors"
                >
                    <div class="aspect-video bg-black/40 overflow-hidden flex items-center justify-center">
                        <img
                            v-if="thumbs[keyOf(m)]"
                            :src="thumbs[keyOf(m)]"
                            :alt="m.name"
                            class="w-full h-full object-cover"
                        />
                        <div v-else class="text-[10px] text-neutral-600 uppercase">
                            {{ m.has_levelshot ? '…' : 'no levelshot' }}
                        </div>
                    </div>
                    <div class="p-2 flex-1 flex flex-col">
                        <div class="text-sm font-semibold text-neutral-100 truncate" :title="m.name">{{ m.name }}</div>
                        <div class="text-[10px] text-neutral-500 truncate mt-0.5" :title="m.pk3">{{ m.pk3 }}</div>

                        <div class="flex items-center gap-1.5 mt-2 pt-2 border-t border-white/[0.06]">
                            <span class="text-[10px] uppercase tracking-wider text-neutral-600 mr-0.5">Run</span>
                            <button
                                class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                :disabled="!config.config.engine_path || !!runningKey"
                                :title="`Run ${m.name} offline in VQ3`"
                                @click="runOffline(m, 'vq3')"
                            >{{ isRunning(m, 'vq3') ? '…' : 'VQ3' }}</button>
                            <button
                                class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                :disabled="!config.config.engine_path || !!runningKey"
                                :title="`Run ${m.name} offline in CPM`"
                                @click="runOffline(m, 'cpm')"
                            >{{ isRunning(m, 'cpm') ? '…' : 'CPM' }}</button>
                        </div>
                    </div>
                </li>
            </ul>
        </div>

        <footer
            v-if="data && data.last_page > 1"
            class="px-5 py-2 border-t border-white/10 flex items-center justify-between text-xs"
        >
            <button
                class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                :disabled="data.current_page <= 1 || loading"
                @click="setPage(data!.current_page - 1)"
            >← Prev</button>
            <span class="text-neutral-500">page {{ data.current_page }} / {{ data.last_page }}</span>
            <button
                class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                :disabled="data.current_page >= data.last_page || loading"
                @click="setPage(data!.current_page + 1)"
            >Next →</button>
        </footer>
    </div>
</template>
