<script setup lang="ts">
    // Live server browser. Mirrors what defrag.racing/servers shows in
    // the browser - same filters, same gametype-detection logic - with
    // per-user PB + rank for the token owner overlaid. Polls every 60s
    // while mounted; manual Refresh covers the "I just changed map,
    // show me now" case.
    //
    // The backend payload is owned by Laravel ServerListService. We
    // type the columns we render in lib/tauri.ts and pass the rest
    // through, so a new backend column doesn't force a launcher
    // release.

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { tauri, type DefragServer } from '../lib/tauri';
    import { q3ToHtml } from '../lib/q3color';
    import { useConfigStore } from '../stores/config';
    import { openExternal } from '../lib/open';

    const config = useConfigStore();

    const servers = ref<DefragServer[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const lastFetchedAt = ref<Date | null>(null);

    // Filter state. Defaults loaded from localStorage so filters
    // survive across launches (same pattern as the website's
    // /servers page).
    type GametypeFilter = 'all' | 'run' | 'ctf' | 'freestyle' | 'teamrun';
    type PhysicsFilter = 'all' | 'vq3' | 'cpm';
    type SortKey = 'popularity' | 'alphabetical';
    type SortOrder = 'asc' | 'desc';

    const STORAGE_KEY = 'launcher.servers.filters.v1';
    const stored = (() => {
        try { return JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}'); }
        catch { return {}; }
    })();

    const search = ref<string>(stored.search ?? '');
    const gametype = ref<GametypeFilter>(stored.gametype ?? 'all');
    const physics = ref<PhysicsFilter>(stored.physics ?? 'all');
    const hideEmpty = ref<boolean>(stored.hideEmpty ?? false);
    const sortKey = ref<SortKey>(stored.sortKey ?? 'popularity');
    const sortOrder = ref<SortOrder>(stored.sortOrder ?? 'desc');

    watch([search, gametype, physics, hideEmpty, sortKey, sortOrder], () => {
        localStorage.setItem(STORAGE_KEY, JSON.stringify({
            search: search.value,
            gametype: gametype.value,
            physics: physics.value,
            hideEmpty: hideEmpty.value,
            sortKey: sortKey.value,
            sortOrder: sortOrder.value,
        }));
    });

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

    const startPolling = () => {
        stopPolling();
        if (!config.hasToken) return;
        pollTimer = window.setInterval(() => {
            if (!document.hidden) fetchServers();
        }, POLL_INTERVAL_MS);
    };
    const stopPolling = () => {
        if (pollTimer !== undefined) { window.clearInterval(pollTimer); pollTimer = undefined; }
    };
    const onVisibility = () => {
        if (document.hidden) stopPolling();
        else if (config.hasToken) { fetchServers(); startPolling(); }
    };

    onMounted(() => {
        if (config.hasToken) {
            fetchServers();
            startPolling();
        }
        document.addEventListener('visibilitychange', onVisibility);
    });
    onActivated(() => {
        if (config.hasToken) {
            fetchServers();
            startPolling();
        }
    });
    onDeactivated(() => stopPolling());
    onUnmounted(() => {
        stopPolling();
        document.removeEventListener('visibilitychange', onVisibility);
    });

    // -- Server classification helpers --------------------------------
    // These mirror filteredAndSortedServers on the web's /servers page;
    // any behavior change should land in both places.

    /** Defrag physics is encoded as a string like "mdf.vq3.run" or
     *  "df.cpm". Reduce to either "vq3" or "cpm". */
    const physicsOf = (s: DefragServer): 'vq3' | 'cpm' => {
        return s.defrag?.toLowerCase().includes('cpm') ? 'cpm' : 'vq3';
    };

    const playerCount = (s: DefragServer): number => s.online_players?.length ?? 0;

    /** Strip Q3 color codes (^1, ^x12, ^123456) from a name so search
     *  matches what the user sees, not the raw control-char string. */
    const stripColors = (s: string): string =>
        s.replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '');

    /** Effective gametype with the same fallback logic the website uses:
     *  start from the scraper-set `type`, then refine by server name
     *  keywords (some admins set type='run' but name contains 'fastcap'
     *  / 'freestyle' / 'teamrun').
     *
     *  `type` is not a clean vocabulary - it carries 'teamruns' next to
     *  'team', and physics values where a gametype belongs - so it is
     *  normalised first. Without that the servers typed 'cpm' or 'vq3'
     *  matched no tab at all and the 'teamruns' ones were invisible to
     *  the team tab.
     *
     *  `mixed` used to read defrag_gametype === '5', on the basis that 5
     *  means run and teamrun at once. It is 5 on every server that exists
     *  - 76 of 76 in production - so it carried no information, and being
     *  OR'd into both the run and teamrun cases it made those two tabs
     *  return the whole list. The name keyword and type='mixed' are what
     *  admins actually set. */
    const classifyServer = (s: DefragServer): { effectiveType: string; mixed: boolean } => {
        const serverType = (s.type ?? 'run').toLowerCase();
        const serverName = stripColors(s.name ?? '').toLowerCase();
        const mixed = serverType === 'mixed' || serverName.includes('mixed');

        let effectiveType = serverType;
        if (effectiveType === 'teamruns') effectiveType = 'team';
        if (effectiveType === 'cpm' || effectiveType === 'vq3' || effectiveType === 'mixed') effectiveType = 'run';

        if (effectiveType === 'run') {
            if (serverName.includes('fastcap') || serverName.includes('ctf')) effectiveType = 'fastcaps';
            else if (serverName.includes('freestyle')) effectiveType = 'freestyle';
            else if (serverName.includes('teamrun') || serverName.includes('team run')) effectiveType = 'team';
        }
        return { effectiveType, mixed };
    };

    const matchesGametype = (s: DefragServer): boolean => {
        if (gametype.value === 'all') return true;
        const { effectiveType, mixed } = classifyServer(s);
        switch (gametype.value) {
            case 'run':       return effectiveType === 'run' || mixed;
            case 'ctf':       return effectiveType === 'ctf' || effectiveType === 'fastcaps';
            case 'freestyle': return effectiveType === 'freestyle';
            // Not `mixed`: a teamrun can be voted on any mixed server, but
            // someone filtering here wants the servers set up for them.
            case 'teamrun':   return effectiveType === 'teamrun' || effectiveType === 'team';
        }
    };

    /** Short uppercase tag for the gametype, rendered next to the
     *  server name so the user sees what kind of server it is at a
     *  glance without having to read the name keywords. */
    const gametypeTag = (s: DefragServer): string => {
        const { effectiveType, mixed } = classifyServer(s);
        // The specific type wins over MIXED. The old order put MIXED first,
        // and since every server counted as mixed, every server was tagged
        // MIXED and the tag told you nothing.
        switch (effectiveType) {
            case 'ctf':
            case 'fastcaps':  return 'CTF';
            case 'freestyle': return 'FS';
            case 'teamrun':
            case 'team':      return 'TEAM';
            default:          return mixed ? 'MIXED' : 'RUN';
        }
    };

    const filteredServers = computed(() => {
        const q = search.value.trim().toLowerCase();
        let result = servers.value.filter((s) => {
            if (hideEmpty.value && playerCount(s) === 0) return false;
            if (physics.value !== 'all' && physicsOf(s) !== physics.value) return false;
            if (!matchesGametype(s)) return false;
            if (q) {
                const haystack = [
                    stripColors(s.name ?? ''),
                    s.plain_name ?? '',
                    s.map,
                    s.ip,
                    `${s.ip}:${s.port}`,
                ].join(' ').toLowerCase();
                if (!haystack.includes(q)) return false;
            }
            return true;
        });

        result = [...result].sort((a, b) => {
            let cmp = 0;
            if (sortKey.value === 'popularity') {
                cmp = playerCount(b) - playerCount(a);
            } else {
                const nameA = (a.plain_name || stripColors(a.name || '')).toLowerCase();
                const nameB = (b.plain_name || stripColors(b.name || '')).toLowerCase();
                cmp = nameA.localeCompare(nameB);
            }
            return sortOrder.value === 'desc' ? cmp : -cmp;
        });

        return result;
    });

    const toggleSort = (key: SortKey) => {
        if (sortKey.value === key) {
            sortOrder.value = sortOrder.value === 'desc' ? 'asc' : 'desc';
        } else {
            sortKey.value = key;
            sortOrder.value = 'desc';
        }
    };

    const resetFilters = () => {
        search.value = '';
        gametype.value = 'all';
        physics.value = 'all';
        hideEmpty.value = false;
        sortKey.value = 'popularity';
        sortOrder.value = 'desc';
    };

    const connect = async (s: DefragServer) => {
        try {
            await tauri.handleProtocolUrl(
                `defrag://${s.ip}:${s.port}`,
                {
                    map: s.map ?? null,
                    server_name: s.name || s.plain_name || null,
                    physics: physicsOf(s),
                },
                'servers',
            );
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Connect failed';
        }
    };

    const openMap = (mapname: string) => {
        if (!mapname) return;
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => { /* best effort */ });
    };

    /** Backend stores `/storage/...`-relative thumbnails; absolute
     *  HTTP URLs are accepted as-is. Returns null when no thumbnail
     *  so the placeholder UI shows instead. */
    const thumbnailUrl = (s: DefragServer): string | null => {
        const t = s.mapdata?.thumbnail;
        if (!t) return null;
        if (t.startsWith('http://') || t.startsWith('https://')) return t;
        return `https://defrag.racing/storage/${t}`;
    };

    const flagUrl = (country: string | null | undefined): string | null => {
        if (!country) return null;
        if (country === '_404' || country === 'XX') return null;
        return `https://defrag.racing/images/flags/${country.toLowerCase()}.png`;
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

    const lastFetchedLabel = computed(() => {
        void _now.value; // re-evaluate every tick
        if (!lastFetchedAt.value) return '';
        const sec = Math.round((Date.now() - lastFetchedAt.value.getTime()) / 1000);
        if (sec < 5) return 'just now';
        if (sec < 60) return `${sec}s ago`;
        const m = Math.floor(sec / 60);
        return `${m}m ago`;
    });

    const summary = computed(() => {
        const total = servers.value.length;
        const totalPlayers = servers.value.reduce((s, x) => s + playerCount(x), 0);
        return { total, shown: filteredServers.value.length, totalPlayers };
    });
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Servers</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    Live list from defrag.racing.
                    <span v-if="summary.total">
                        {{ summary.shown }} of {{ summary.total }} shown · {{ summary.totalPlayers }} player{{ summary.totalPlayers === 1 ? '' : 's' }} online
                    </span>
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs text-neutral-500 flex-shrink-0">
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
                </p>
                <RouterLink
                    :to="{ name: 'settings', query: { highlight: 'token' } }"
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                >Open Settings to paste a token →</RouterLink>
            </div>
        </div>

        <template v-else>
            <!-- Filter bar. Pills are grouped by purpose: search +
                 [gametype] + [physics] + [hide empty] + [sort] + reset.
                 Same mental model as the web /servers page. -->
            <div class="px-5 py-2 border-b border-white/10 space-y-2 text-xs">
                <div class="flex items-center gap-2 flex-wrap">
                    <input
                        v-model="search"
                        type="text"
                        placeholder="Search by name, map, IP…"
                        class="flex-1 min-w-[180px] bg-black/60 border border-white/10 rounded px-2 py-1.5 text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                    />
                    <label class="flex items-center gap-1.5 text-neutral-400 cursor-pointer select-none px-2">
                        <input type="checkbox" v-model="hideEmpty" class="accent-brand-500" />
                        Hide empty
                    </label>
                    <button
                        class="px-2 py-1 rounded text-neutral-500 hover:text-neutral-200"
                        @click="resetFilters"
                        title="Clear all filters"
                    >Reset</button>
                </div>

                <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-neutral-500 mr-1">Gametype:</span>
                    <div class="flex bg-white/5 rounded overflow-hidden">
                        <button
                            v-for="opt in [
                                { v: 'all', label: 'All' },
                                { v: 'run', label: 'Run' },
                                { v: 'ctf', label: 'CTF' },
                                { v: 'freestyle', label: 'Freestyle' },
                                { v: 'teamrun', label: 'Team' },
                            ] as const"
                            :key="opt.v"
                            class="px-2.5 py-1 transition-colors"
                            :class="gametype === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200'"
                            @click="gametype = opt.v"
                        >{{ opt.label }}</button>
                    </div>

                    <span class="text-neutral-500 ml-2 mr-1">Physics:</span>
                    <div class="flex bg-white/5 rounded overflow-hidden">
                        <button
                            v-for="opt in ['all', 'vq3', 'cpm'] as const"
                            :key="opt"
                            class="px-2.5 py-1 transition-colors"
                            :class="physics === opt ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200'"
                            @click="physics = opt"
                        >{{ opt.toUpperCase() }}</button>
                    </div>

                    <span class="text-neutral-500 ml-2 mr-1">Sort:</span>
                    <div class="flex bg-white/5 rounded overflow-hidden">
                        <button
                            v-for="opt in [
                                { v: 'popularity', label: 'Players' },
                                { v: 'alphabetical', label: 'Name' },
                            ] as const"
                            :key="opt.v"
                            class="px-2.5 py-1 transition-colors flex items-center gap-1"
                            :class="sortKey === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200'"
                            @click="toggleSort(opt.v)"
                        >
                            {{ opt.label }}
                            <span v-if="sortKey === opt.v" class="text-[10px]">
                                {{ sortOrder === 'desc' ? '↓' : '↑' }}
                            </span>
                        </button>
                    </div>
                </div>
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
                        class="px-5 py-3"
                    >
                        <div class="flex items-start gap-3">
                            <!-- Map thumbnail. Click opens the map page on
                                 defrag.racing in the system browser. -->
                            <button
                                class="w-20 h-14 rounded bg-black/40 border border-white/10 overflow-hidden flex-shrink-0 hover:border-brand-500/40"
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
                                <div class="flex items-center gap-2 min-w-0">
                                    <img
                                        v-if="flagUrl(s.location)"
                                        :src="flagUrl(s.location)!"
                                        :alt="s.location || ''"
                                        class="w-4 h-3 rounded flex-shrink-0"
                                        @error="($event.target as HTMLImageElement).style.display='none'"
                                    />
                                    <div
                                        class="text-sm text-neutral-100 truncate font-semibold"
                                        :title="s.plain_name || stripColors(s.name)"
                                        v-html="q3ToHtml(s.name || s.plain_name)"
                                    ></div>
                                    <span class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-brand-500/15 text-brand-300 flex-shrink-0">
                                        {{ gametypeTag(s) }}
                                    </span>
                                </div>
                                <div class="text-xs text-neutral-500 truncate flex items-center gap-2 mt-0.5">
                                    <button
                                        class="text-brand-400 hover:underline"
                                        @click="openMap(s.map)"
                                    >{{ s.map }}</button>
                                    <span class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5">{{ physicsOf(s) }}</span>
                                    <span class="text-neutral-600">·</span>
                                    <span class="font-mono">{{ s.ip }}:{{ s.port }}</span>
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
                                <!-- Map record holder for context. Skipped when
                                     the server doesn't report a besttime. -->
                                <div v-if="s.besttime_time && s.besttime_name" class="text-xs text-yellow-300/70 mt-0.5 flex items-center gap-1">
                                    <span class="text-yellow-500">★</span>
                                    <span>{{ formatTime(s.besttime_time) }}</span>
                                    <span class="text-neutral-500">by</span>
                                    <img
                                        v-if="flagUrl(s.besttime_country)"
                                        :src="flagUrl(s.besttime_country)!"
                                        :alt="s.besttime_country || ''"
                                        class="w-3 h-2 rounded flex-shrink-0"
                                        @error="($event.target as HTMLImageElement).style.display='none'"
                                    />
                                    <span
                                        class="truncate"
                                        :title="stripColors(s.besttime_name || '')"
                                        v-html="q3ToHtml(s.besttime_name)"
                                    ></span>
                                </div>
                            </div>

                            <div class="flex flex-col items-end gap-1 flex-shrink-0">
                                <div class="text-xs text-neutral-400 whitespace-nowrap">
                                    <span class="text-neutral-100 font-semibold text-sm">{{ playerCount(s) }}</span>
                                    player{{ playerCount(s) === 1 ? '' : 's' }}
                                </div>
                                <button
                                    class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-xs font-semibold"
                                    @click="connect(s)"
                                >Connect</button>
                            </div>
                        </div>

                        <!-- Player list. Indented under the thumbnail so the
                             row reads "this server, with these people on it"
                             at a glance. Hidden on empty servers - the
                             player count above already says zero. Players
                             only; spectators would crowd the row and the
                             "is it busy" question is answered by player
                             count alone. -->
                        <div
                            v-if="(s.online_players?.length ?? 0) > 0"
                            class="ml-[5.75rem] mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs"
                        >
                            <span
                                v-for="(p, idx) in (s.online_players ?? [])"
                                :key="`${s.ip}:${s.port}#${idx}`"
                                class="flex items-center gap-1 max-w-[14rem] min-w-0"
                            >
                                <img
                                    v-if="flagUrl(p.country)"
                                    :src="flagUrl(p.country)!"
                                    :alt="p.country || ''"
                                    class="w-3 h-2 rounded flex-shrink-0"
                                    @error="($event.target as HTMLImageElement).style.display='none'"
                                />
                                <span
                                    class="truncate"
                                    :title="p.plain_name || stripColors(p.name)"
                                    v-html="q3ToHtml(p.name || p.plain_name)"
                                ></span>
                            </span>
                        </div>
                    </li>
                </ul>
            </div>
        </template>
    </div>
</template>
