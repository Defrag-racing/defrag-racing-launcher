<script setup lang="ts">
    // Map browser. Paginated list of maps newest first with a simple
    // name search; advanced filtering (weapons / gametype / NSFW
    // toggle / item filtering) stays on the web's /maps page, which
    // any thumbnail / name click jumps to with the full filter UI.

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { tauri, type MapRow, type Paginated } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openExternal } from '../lib/open';

    const config = useConfigStore();

    const search = ref('');
    const page = ref(1);
    const data = ref<Paginated<MapRow> | null>(null);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const lastFetchedAt = ref<Date | null>(null);

    const load = async () => {
        if (!config.hasToken) return;
        loading.value = true;
        error.value = null;
        try {
            data.value = await tauri.getMaps(page.value, search.value);
            lastFetchedAt.value = new Date();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to load maps';
        } finally {
            loading.value = false;
        }
    };

    // Debounce search input - typing should feel live but we don't
    // want a request per keystroke. 350ms is the threshold most users
    // hit "I'm done typing" within without feeling laggy.
    let searchTimer: number | undefined;
    watch(search, () => {
        if (searchTimer !== undefined) window.clearTimeout(searchTimer);
        searchTimer = window.setTimeout(() => {
            page.value = 1;
            void load();
        }, 350);
    });

    // Auto-refresh while the tab is active. Map list rarely changes in
    // a minute window but a longer interval avoids re-pulling 50 rows
    // for a user just clicking around. Visibility-aware so a tray-
    // hidden window doesn't keep hammering the endpoint.
    const POLL_MS = 120_000;
    let pollTimer: number | undefined;
    const startPolling = () => {
        stopPolling();
        pollTimer = window.setInterval(() => {
            if (!document.hidden) void load();
        }, POLL_MS);
    };
    const stopPolling = () => {
        if (pollTimer !== undefined) { window.clearInterval(pollTimer); pollTimer = undefined; }
    };
    const onVisibility = () => {
        if (document.hidden) stopPolling();
        else { void load(); startPolling(); }
    };

    // Tick a re-render every 5s so the "Updated Xs ago" label stays
    // fresh without a network call.
    const _now = ref(Date.now());
    let labelTimer: number | undefined;
    const lastFetchedLabel = computed(() => {
        void _now.value; // re-evaluate every tick
        if (!lastFetchedAt.value) return '';
        const sec = Math.round((Date.now() - lastFetchedAt.value.getTime()) / 1000);
        if (sec < 5) return 'just now';
        if (sec < 60) return `${sec}s ago`;
        const m = Math.floor(sec / 60);
        return `${m}m ago`;
    });

    onMounted(() => {
        void load();
        startPolling();
        labelTimer = window.setInterval(() => { _now.value = Date.now(); }, 5000);
        document.addEventListener('visibilitychange', onVisibility);
    });
    onActivated(() => {
        void load();
        startPolling();
    });
    onDeactivated(() => stopPolling());
    onUnmounted(() => {
        stopPolling();
        if (labelTimer !== undefined) window.clearInterval(labelTimer);
        document.removeEventListener('visibilitychange', onVisibility);
        if (searchTimer !== undefined) window.clearTimeout(searchTimer);
    });

    const setPage = (p: number) => {
        if (p < 1 || (data.value?.last_page && p > data.value.last_page)) return;
        page.value = p;
        void load();
    };

    const thumbnailUrl = (m: MapRow): string | null => {
        const t = m.thumbnail;
        if (!t) return null;
        if (t.startsWith('http://') || t.startsWith('https://')) return t;
        return `https://defrag.racing/storage/${t}`;
    };

    const openMap = (name: string) => {
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(name)}`)
            .catch(() => { /* best effort */ });
    };

    // Launch the engine to run a map offline in the chosen physics. Uses
    // the same launch plumbing as developer-mode profiles - the engine
    // reads `+vq3 <map>` / `+cpm <map>` as a startup console command. Map
    // name is quoted so the arg splitter keeps it one token. Failures
    // surface in the existing top-of-view error banner.
    const runOffline = async (name: string, physics: 'vq3' | 'cpm') => {
        error.value = null;
        try {
            await tauri.launchEngineArgs(`+${physics} "${name}"`);
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to launch the engine';
        }
    };

    const formatDate = (s: string | null): string => {
        if (!s) return '';
        const d = new Date(s.replace(' ', 'T') + 'Z');
        return isNaN(d.getTime()) ? s : d.toLocaleDateString();
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Maps</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    Browse maps from defrag.racing, newest first. Click a map to
                    open its full page on the web for weapons / records / demos.
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs text-neutral-500 flex-shrink-0">
                <span v-if="lastFetchedAt">Updated {{ lastFetchedLabel }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="load"
                >{{ loading ? 'Loading…' : 'Refresh' }}</button>
            </div>
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    Maps browser needs a token from your defrag.racing account.
                </p>
                <RouterLink
                    :to="{ name: 'settings', query: { highlight: 'token' } }"
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                >Open Settings to paste a token →</RouterLink>
            </div>
        </div>

        <template v-else>
            <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 text-xs">
                <input
                    v-model="search"
                    type="text"
                    placeholder="Search map name…"
                    class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                />
                <div v-if="data" class="text-neutral-500 whitespace-nowrap">
                    page {{ data.current_page }} / {{ data.last_page }} · {{ data.total }} total
                </div>
            </div>

            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <div class="flex-1 overflow-auto">
                <div v-if="loading && !data" class="p-8 text-center text-sm text-neutral-500">
                    Loading…
                </div>
                <div v-else-if="data && !data.data.length" class="p-8 text-center text-sm text-neutral-500">
                    No maps match this search.
                </div>
                <ul v-else-if="data" class="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 gap-3 p-3">
                    <li
                        v-for="m in data.data"
                        :key="m.id"
                        class="bg-neutral-900/40 border border-white/10 rounded-lg overflow-hidden flex flex-col hover:border-brand-500/40 transition-colors"
                    >
                        <button
                            class="aspect-video bg-black/40 overflow-hidden flex items-center justify-center"
                            :title="`Open ${m.name} on defrag.racing`"
                            @click="openMap(m.name)"
                        >
                            <img
                                v-if="thumbnailUrl(m)"
                                :src="thumbnailUrl(m)!"
                                :alt="m.name"
                                class="w-full h-full object-cover"
                                loading="lazy"
                            />
                            <div v-else class="text-[10px] text-neutral-600 uppercase">
                                no thumbnail
                            </div>
                        </button>
                        <div class="p-2 flex-1 flex flex-col">
                            <button
                                class="text-sm font-semibold text-neutral-100 truncate text-left hover:text-brand-300"
                                @click="openMap(m.name)"
                            >{{ m.name }}</button>
                            <div class="text-xs text-neutral-500 truncate mt-0.5" v-if="m.author">
                                by {{ m.author }}
                            </div>
                            <div class="flex items-center gap-2 mt-1 text-[10px] text-neutral-500">
                                <span v-if="m.physics" class="uppercase px-1 py-0.5 rounded bg-white/5 text-neutral-300">{{ m.physics }}</span>
                                <span v-if="m.gametype" class="uppercase px-1 py-0.5 rounded bg-white/5 text-neutral-300">{{ m.gametype }}</span>
                                <span v-if="m.is_nsfw" class="uppercase px-1 py-0.5 rounded bg-red-500/15 text-red-300">NSFW</span>
                                <span class="ml-auto whitespace-nowrap">{{ formatDate(m.date_added) }}</span>
                            </div>

                            <!-- Run the map offline in the chosen physics. Both
                                 buttons appear on every card; the engine path
                                 is required (same gating as Quick launch). -->
                            <div class="flex items-center gap-1.5 mt-2 pt-2 border-t border-white/[0.06]">
                                <span class="text-[10px] uppercase tracking-wider text-neutral-600 mr-0.5">Run offline</span>
                                <button
                                    class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                    :disabled="!config.config.engine_path"
                                    :title="config.config.engine_path
                                        ? `Run ${m.name} offline in VQ3`
                                        : 'Pick an engine in Settings first'"
                                    @click="runOffline(m.name, 'vq3')"
                                >VQ3</button>
                                <button
                                    class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                    :disabled="!config.config.engine_path"
                                    :title="config.config.engine_path
                                        ? `Run ${m.name} offline in CPM`
                                        : 'Pick an engine in Settings first'"
                                    @click="runOffline(m.name, 'cpm')"
                                >CPM</button>
                            </div>
                        </div>
                    </li>
                </ul>
            </div>

            <footer
                v-if="data && (data.last_page ?? 1) > 1"
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
                    :disabled="data.current_page >= (data.last_page ?? 1) || loading"
                    @click="setPage(data!.current_page + 1)"
                >Next →</button>
            </footer>
        </template>
    </div>
</template>
