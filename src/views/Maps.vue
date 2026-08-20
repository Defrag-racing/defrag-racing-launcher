<script setup lang="ts">
    // Map browser. Paginated list of maps newest first with a simple
    // name search; advanced filtering (weapons / gametype / NSFW
    // toggle / item filtering) stays on the web's /maps page, which
    // any thumbnail / name click jumps to with the full filter UI.

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { useOnScreen } from '../lib/visibility';
    import { tauri, type MapRow, type Paginated } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { t } from '../lib/i18n';
    import { openExternal } from '../lib/open';
    import {
        splitCodes,
        weaponIcon, weaponName,
        itemIcon, itemName,
        functionIcon, functionName,
    } from '../lib/mapIcons';
    import MapsOffline from './MapsOffline.vue';

    const config = useConfigStore();

    // Online (defrag.racing browser) vs Offline (maps installed locally in
    // baseq3). Online is token-gated; offline works without a token.
    const subtab = ref<'online' | 'offline'>('online');

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
            error.value = e?.toString?.() ?? t('Failed to load maps');
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
    const onScreen = useOnScreen();

    const POLL_MS = 120_000;
    let pollTimer: number | undefined;
    const startPolling = () => {
        stopPolling();
        pollTimer = window.setInterval(() => {
            if (onScreen.value) void load();
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

    // Run a map offline in the chosen physics. The backend first ensures
    // the map's pk3 is in baseq3 (downloading it by its ORIGINAL pk3 name
    // if missing - one pk3 can hold several maps), then launches
    // `+vq3 <map>` / `+cpm <map>`. While a card is busy we show a spinner
    // and block repeat clicks; failures surface in the top error banner.
    const runningKey = ref<string | null>(null);
    const keyOf = (id: number, physics: string) => `${id}:${physics}`;
    const runOffline = async (m: MapRow, physics: 'vq3' | 'cpm') => {
        const k = keyOf(m.id, physics);
        if (runningKey.value) return;
        runningKey.value = k;
        error.value = null;
        try {
            await tauri.runMapOffline(m.name, physics, m.pk3);
        } catch (e: any) {
            error.value = e?.toString?.() ?? t('Failed to run the map');
        } finally {
            runningKey.value = null;
        }
    };
    const isRunning = (id: number, physics: string) => runningKey.value === keyOf(id, physics);

    // Hide an icon whose SVG is missing (unknown code with no bundled
    // file) instead of showing a broken-image glyph over the thumbnail.
    const onIconError = (e: Event) => {
        (e.target as HTMLImageElement).style.display = 'none';
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
            <div class="min-w-0 flex items-center gap-3">
                <div class="font-semibold">{{ $t('Maps') }}</div>
                <!-- Online / Offline sub-tabs -->
                <div class="flex items-center gap-1 text-xs">
                    <button
                        class="px-2 py-1 rounded transition-colors"
                        :class="subtab === 'online' ? 'bg-white/10 text-neutral-100 font-semibold' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                        @click="subtab = 'online'"
                    >{{ $t('Online') }}</button>
                    <button
                        class="px-2 py-1 rounded transition-colors"
                        :class="subtab === 'offline' ? 'bg-white/10 text-neutral-100 font-semibold' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                        @click="subtab = 'offline'"
                    >{{ $t('Offline') }} <span class="text-neutral-500">{{ $t('(on this PC)') }}</span></button>
                </div>
            </div>
            <div v-if="subtab === 'online'" class="flex items-center gap-2 text-xs text-neutral-500 flex-shrink-0">
                <span v-if="lastFetchedAt">{{ $t('Updated') }} {{ lastFetchedLabel }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="load"
                >{{ loading ? $t('Loading…') : $t('Refresh') }}</button>
            </div>
        </header>

        <template v-if="subtab === 'online'">
        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">{{ $t('Token required') }}</div>
                <p class="text-sm text-neutral-500">
                    {{ $t('The maps browser needs a token from your defrag.racing account.') }}
                </p>
                <RouterLink
                    :to="{ name: 'settings', query: { highlight: 'token' } }"
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                >{{ $t('Open Settings to paste a token →') }}</RouterLink>
            </div>
        </div>

        <template v-else>
            <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 text-xs">
                <input
                    v-model="search"
                    type="text"
                    :placeholder="$t('Search map name…')"
                    class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                />
                <div v-if="data" class="text-neutral-500 whitespace-nowrap">
                    {{ $t('page :n of :total', { n: data.current_page, total: data.last_page ?? 1 }) }} · {{ $t(':count in total', { count: data.total ?? 0 }) }}
                </div>
            </div>

            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <div class="flex-1 overflow-auto">
                <div v-if="loading && !data" class="p-8 text-center text-sm text-neutral-500">
                    {{ $t('Loading…') }}
                </div>
                <div v-else-if="data && !data.data.length" class="p-8 text-center text-sm text-neutral-500">
                    {{ $t('No maps match this search.') }}
                </div>
                <ul v-else-if="data" class="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-4 gap-3 p-3">
                    <li
                        v-for="m in data.data"
                        :key="m.id"
                        class="bg-neutral-900/40 border border-white/10 rounded-lg overflow-hidden flex flex-col hover:border-brand-500/40 transition-colors"
                    >
                        <button
                            class="relative aspect-video bg-black/40 overflow-hidden flex items-center justify-center"
                            :title="$t('Open :map on defrag.racing', { map: m.name })"
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
                                {{ $t('no thumbnail') }}
                            </div>

                            <!-- Weapons / items / functions icons over the
                                 thumbnail (bundled SVGs from defrag.racing).
                                 Stacked bottom-right so they don't cover the
                                 in-image map name (bottom-left). Clicks still
                                 bubble to the button -> open the map page. -->
                            <div class="absolute bottom-1 right-1 flex flex-col items-end gap-0.5">
                                <div v-if="splitCodes(m.weapons).length" class="flex flex-wrap justify-end gap-0.5 max-w-[60%] bg-black/70 rounded px-1 py-0.5">
                                    <img
                                        v-for="c in splitCodes(m.weapons)"
                                        :key="`w-${c}`"
                                        :src="weaponIcon(c)"
                                        :alt="weaponName(c)"
                                        :title="weaponName(c)"
                                        class="w-3.5 h-3.5"
                                        @error="onIconError"
                                    />
                                </div>
                                <div v-if="splitCodes(m.items).length" class="flex flex-wrap justify-end gap-0.5 max-w-[60%] bg-black/70 rounded px-1 py-0.5">
                                    <img
                                        v-for="c in splitCodes(m.items)"
                                        :key="`i-${c}`"
                                        :src="itemIcon(c)"
                                        :alt="itemName(c)"
                                        :title="itemName(c)"
                                        class="w-3.5 h-3.5"
                                        @error="onIconError"
                                    />
                                </div>
                                <div v-if="splitCodes(m.functions).length" class="flex flex-wrap justify-end gap-0.5 max-w-[70%] bg-black/70 rounded px-1 py-0.5">
                                    <img
                                        v-for="c in splitCodes(m.functions)"
                                        :key="`f-${c}`"
                                        :src="functionIcon(c)"
                                        :alt="functionName(c)"
                                        :title="functionName(c)"
                                        class="w-3.5 h-3.5"
                                        @error="onIconError"
                                    />
                                </div>
                            </div>
                        </button>
                        <div class="p-2 flex-1 flex flex-col">
                            <button
                                class="text-sm font-semibold text-neutral-100 truncate text-left hover:text-brand-300"
                                @click="openMap(m.name)"
                            >{{ m.name }}</button>
                            <div class="text-xs text-neutral-500 truncate mt-0.5" v-if="m.author">
                                {{ $t('by :author', { author: m.author }) }}
                            </div>
                            <div class="flex items-center gap-2 mt-1 text-[10px] text-neutral-500">
                                <span v-if="m.physics" class="uppercase px-1 py-0.5 rounded bg-white/5 text-neutral-300">{{ m.physics }}</span>
                                <span v-if="m.gametype" class="uppercase px-1 py-0.5 rounded bg-white/5 text-neutral-300">{{ m.gametype }}</span>
                                <span v-if="m.is_nsfw" class="uppercase px-1 py-0.5 rounded bg-red-500/15 text-red-300">NSFW</span>
                                <span class="ml-auto whitespace-nowrap">{{ formatDate(m.date_added) }}</span>
                            </div>

                            <!-- Run the map offline in the chosen physics. Both
                                 buttons appear on every card; the engine path
                                 is required (same gating as Quick launch). The
                                 backend auto-downloads the map's pk3 into
                                 baseq3 first if it isn't installed. Label sits
                                 on its own line so it stays readable and the
                                 buttons keep their full width. -->
                            <div class="mt-2 pt-2 border-t border-white/[0.06]">
                                <div class="text-[10px] uppercase tracking-wider text-neutral-400 mb-1 flex items-center gap-1">
                                    <span class="text-emerald-400">▶</span>
                                    {{ $t('Click to run offline instantly') }}
                                </div>
                                <div class="flex items-center gap-1.5">
                                    <button
                                        class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                        :disabled="!config.config.engine_path || !!runningKey"
                                        :title="config.config.engine_path
                                            ? $t('Run :map offline in :physics - downloads the map if it is missing', { map: m.name, physics: 'VQ3' })
                                            : $t('Pick an engine in Settings first')"
                                        @click="runOffline(m, 'vq3')"
                                    >{{ isRunning(m.id, 'vq3') ? '…' : 'VQ3' }}</button>
                                    <button
                                        class="flex-1 px-2 py-1 rounded text-[11px] font-semibold bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                                        :disabled="!config.config.engine_path || !!runningKey"
                                        :title="config.config.engine_path
                                            ? $t('Run :map offline in :physics - downloads the map if it is missing', { map: m.name, physics: 'CPM' })
                                            : $t('Pick an engine in Settings first')"
                                        @click="runOffline(m, 'cpm')"
                                    >{{ isRunning(m.id, 'cpm') ? '…' : 'CPM' }}</button>
                                </div>
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
                >{{ $t('← Prev') }}</button>
                <span class="text-neutral-500">{{ $t('page :n of :total', { n: data.current_page, total: data.last_page ?? 1 }) }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                    :disabled="data.current_page >= (data.last_page ?? 1) || loading"
                    @click="setPage(data!.current_page + 1)"
                >{{ $t('Next →') }}</button>
            </footer>
        </template>
        </template>

        <MapsOffline v-else />
    </div>
</template>
