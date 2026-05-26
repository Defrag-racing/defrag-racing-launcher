<script setup lang="ts">
    // Map browser. Paginated list of maps newest first with a simple
    // name search; advanced filtering (weapons / gametype / NSFW
    // toggle / item filtering) stays on the web's /maps page, which
    // any thumbnail / name click jumps to with the full filter UI.

    import { onMounted, onUnmounted, ref, watch } from 'vue';
    import { tauri, type MapRow, type Paginated } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const search = ref('');
    const page = ref(1);
    const data = ref<Paginated<MapRow> | null>(null);
    const loading = ref(false);
    const error = ref<string | null>(null);

    const load = async () => {
        if (!config.hasToken) return;
        loading.value = true;
        error.value = null;
        try {
            data.value = await tauri.getMaps(page.value, search.value);
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

    onMounted(() => {
        void load();
    });
    onUnmounted(() => {
        if (searchTimer !== undefined) window.clearTimeout(searchTimer);
    });

    const setPage = (p: number) => {
        if (p < 1 || (data.value && p > data.value.last_page)) return;
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
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(name)}`)
            .catch(() => { /* best effort */ });
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
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    Maps browser needs a token from your defrag.racing account.
                    Open Settings to paste one.
                </p>
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
        </template>
    </div>
</template>
