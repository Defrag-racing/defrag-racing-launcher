<script setup lang="ts">
    // Live server browser. Mirrors what defrag.racing/servers shows in
    // the browser, with per-user PB + rank for the token owner. Polls
    // every 60s while the view is mounted; a manual Refresh button
    // covers the "I just changed map, show me now" case.
    //
    // The backend payload is owned by Laravel ServerListService - we
    // type the columns we render and pass the rest through.

    import { computed, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type DefragServer } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const servers = ref<DefragServer[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const lastFetchedAt = ref<Date | null>(null);
    const search = ref('');
    const physicsFilter = ref<'all' | 'vq3' | 'cpm'>('all');
    const onlyWithPlayers = ref(false);

    const POLL_INTERVAL_MS = 60_000;
    let pollTimer: number | undefined;

    const fetchServers = async () => {
        // Don't pile up requests if a previous one is still in flight
        // (slow network or laggy backend) - the next poll tick will
        // try again.
        if (loading.value) return;
        loading.value = true;
        error.value = null;
        try {
            const resp = await tauri.getServers();
            servers.value = resp.servers ?? [];
            lastFetchedAt.value = new Date();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to load servers';
        } finally {
            loading.value = false;
        }
    };

    onMounted(() => {
        // Only attempt to fetch if a token is present. Without one the
        // backend returns 401, which we'd just surface as an error -
        // the empty-state UI explains what to do instead.
        if (config.hasToken) {
            fetchServers();
            pollTimer = window.setInterval(fetchServers, POLL_INTERVAL_MS);
        }
    });

    onUnmounted(() => {
        if (pollTimer !== undefined) window.clearInterval(pollTimer);
    });

    /** Defrag physics is encoded as a string like "mdf.vq3.run" or
     *  "df.cpm". Reduce to either "vq3" or "cpm" for the filter pill. */
    const physicsOf = (s: DefragServer): 'vq3' | 'cpm' => {
        return s.defrag?.toLowerCase().includes('cpm') ? 'cpm' : 'vq3';
    };

    const playerCount = (s: DefragServer): number => {
        return (s.onlinePlayers?.length ?? 0);
    };

    const filteredServers = computed(() => {
        const q = search.value.trim().toLowerCase();
        return servers.value.filter((s) => {
            if (onlyWithPlayers.value && playerCount(s) === 0) return false;
            if (physicsFilter.value !== 'all' && physicsOf(s) !== physicsFilter.value) return false;
            if (q) {
                const haystack = [
                    s.plain_name ?? s.name,
                    s.map,
                    s.ip,
                    `${s.ip}:${s.port}`,
                ].join(' ').toLowerCase();
                if (!haystack.includes(q)) return false;
            }
            return true;
        });
    });

    const connect = async (s: DefragServer) => {
        try {
            await tauri.handleProtocolUrl(`defrag://${s.ip}:${s.port}`);
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Connect failed';
        }
    };

    const openMap = (mapname: string) => {
        if (!mapname) return;
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => { /* best effort */ });
    };

    const thumbnailUrl = (s: DefragServer): string | null => {
        const t = s.mapdata?.thumbnail;
        if (!t) return null;
        // Backend stores `/storage/...`-relative thumbnails; absolute
        // any-scheme URLs are also accepted as-is.
        if (t.startsWith('http://') || t.startsWith('https://')) return t;
        return `https://defrag.racing/storage/${t}`;
    };

    /** Defrag stores times as milliseconds; format as "MM:SS.mmm" or
     *  "SS.mmm" depending on length. */
    const formatTime = (ms: number | null | undefined): string => {
        if (ms == null || ms <= 0) return '-';
        const totalSec = Math.floor(ms / 1000);
        const m = Math.floor(totalSec / 60);
        const s = totalSec % 60;
        const mmm = ms % 1000;
        if (m > 0) return `${m}:${s.toString().padStart(2, '0')}.${mmm.toString().padStart(3, '0')}`;
        return `${s}.${mmm.toString().padStart(3, '0')}`;
    };

    const lastFetchedLabel = computed(() => {
        if (!lastFetchedAt.value) return '';
        const sec = Math.round((Date.now() - lastFetchedAt.value.getTime()) / 1000);
        if (sec < 5) return 'just now';
        if (sec < 60) return `${sec}s ago`;
        const m = Math.floor(sec / 60);
        return `${m}m ago`;
    });

    // Tick a re-render every 5s so the "last updated" label stays fresh
    // without a full network call.
    const _now = ref(Date.now());
    let labelTimer: number | undefined;
    onMounted(() => {
        labelTimer = window.setInterval(() => { _now.value = Date.now(); }, 5000);
    });
    onUnmounted(() => {
        if (labelTimer !== undefined) window.clearInterval(labelTimer);
    });
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div>
                <div class="font-semibold">Servers</div>
                <div class="text-xs text-neutral-500 mt-0.5">
                    Live list from defrag.racing. Click <strong class="text-brand-400">Connect</strong> to launch your engine and join.
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs text-neutral-500">
                <span v-if="lastFetchedAt">Updated {{ lastFetchedLabel }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="fetchServers"
                >{{ loading ? 'Loading…' : 'Refresh' }}</button>
            </div>
        </header>

        <!-- No-token state. Without a token the backend would 401 us,
             so we don't even try - clearer message than a generic error. -->
        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    The server browser needs a token from your defrag.racing account.
                    Open Settings to paste one.
                </p>
            </div>
        </div>

        <template v-else>
            <!-- Filters -->
            <div class="px-5 py-2 border-b border-white/10 flex items-center gap-2 flex-wrap text-xs">
                <input
                    v-model="search"
                    type="text"
                    placeholder="Search by name, map, IP…"
                    class="flex-1 min-w-[160px] bg-black/60 border border-white/10 rounded px-2 py-1 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                />
                <div class="flex bg-white/5 rounded overflow-hidden">
                    <button
                        v-for="opt in ['all', 'vq3', 'cpm'] as const"
                        :key="opt"
                        class="px-2 py-1"
                        :class="physicsFilter === opt ? 'bg-brand-500/25 text-brand-200' : 'text-neutral-400 hover:text-neutral-200'"
                        @click="physicsFilter = opt"
                    >{{ opt.toUpperCase() }}</button>
                </div>
                <label class="flex items-center gap-1.5 text-neutral-400 cursor-pointer select-none">
                    <input type="checkbox" v-model="onlyWithPlayers" class="accent-brand-500" />
                    Only with players
                </label>
            </div>

            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <!-- Server list -->
            <div class="flex-1 overflow-auto">
                <div v-if="loading && !servers.length" class="p-8 text-center text-sm text-neutral-500">
                    Loading servers…
                </div>
                <div v-else-if="!filteredServers.length" class="p-8 text-center text-sm text-neutral-500">
                    No servers match the current filter.
                </div>
                <ul v-else class="divide-y divide-white/[0.04]">
                    <li
                        v-for="s in filteredServers"
                        :key="`${s.ip}:${s.port}`"
                        class="px-5 py-3 flex items-start gap-3"
                    >
                        <!-- Map thumbnail. mapdata is eager-loaded by the
                             backend; thumbnailUrl handles both relative
                             /storage/... paths and absolute URLs. -->
                        <button
                            class="w-16 h-12 rounded bg-black/40 border border-white/10 overflow-hidden flex-shrink-0 hover:border-brand-500/40"
                            :title="`Open ${s.map} on defrag.racing`"
                            @click="openMap(s.map)"
                        >
                            <img
                                v-if="thumbnailUrl(s)"
                                :src="thumbnailUrl(s)!"
                                :alt="s.map"
                                class="w-full h-full object-cover"
                                loading="lazy"
                            />
                            <div v-else class="w-full h-full flex items-center justify-center text-[10px] text-neutral-600 uppercase">
                                no map
                            </div>
                        </button>

                        <div class="flex-1 min-w-0">
                            <div class="text-sm text-neutral-100 truncate font-semibold">
                                {{ s.plain_name || s.name }}
                            </div>
                            <div class="text-xs text-neutral-500 truncate flex items-center gap-2">
                                <button
                                    class="text-brand-400 hover:underline"
                                    @click="openMap(s.map)"
                                >{{ s.map }}</button>
                                <span class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5">{{ physicsOf(s) }}</span>
                                <span class="text-neutral-600">·</span>
                                <span>{{ s.ip }}:{{ s.port }}</span>
                            </div>
                            <!-- Per-user PB + rank for the token owner on
                                 this server's current map. Hidden when the
                                 user has no time on it. -->
                            <div v-if="s.mytime_time" class="text-xs text-emerald-300/80 mt-0.5">
                                Your PB: <strong>{{ formatTime(s.mytime_time) }}</strong>
                                <span v-if="s.myrank_position && s.myrank_total" class="text-emerald-300/60 ml-1">
                                    (rank {{ s.myrank_position }} / {{ s.myrank_total }})
                                </span>
                            </div>
                        </div>

                        <div class="flex flex-col items-end gap-1 flex-shrink-0">
                            <div class="text-xs text-neutral-400">
                                <span class="text-neutral-100 font-semibold">{{ playerCount(s) }}</span>
                                player{{ playerCount(s) === 1 ? '' : 's' }}
                            </div>
                            <button
                                class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-xs font-semibold"
                                @click="connect(s)"
                            >Connect</button>
                        </div>
                    </li>
                </ul>
            </div>
        </template>
    </div>
</template>
