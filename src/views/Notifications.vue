<script setup lang="ts">
    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type NotificationsFeed, type SystemNotificationRow, type RecordNotificationRow } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { q3ToHtml } from '../lib/q3color';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const feed = ref<NotificationsFeed | null>(null);
    const loading = ref(true);
    const error = ref<string | null>(null);

    type MainTab = 'records' | 'system';
    type RecordTab = 'all' | 'beaten' | 'worldrecords';
    type SystemTab = 'all' | 'announcements' | 'clan' | 'tournament' | 'profile' | 'render';
    const mainTab = ref<MainTab>('records');
    const recordTab = ref<RecordTab>('all');
    const systemTab = ref<SystemTab>('all');

    const refresh = async () => {
        if (!config.hasToken) { loading.value = false; return; }
        loading.value = true;
        error.value = null;
        try {
            feed.value = await tauri.getNotifications();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to load notifications';
        } finally {
            loading.value = false;
        }
    };

    const POLL_MS = 60_000;
    let pollTimer: number | undefined;
    const startPolling = () => {
        stopPolling();
        pollTimer = window.setInterval(() => {
            if (!document.hidden) refresh();
        }, POLL_MS);
    };
    const stopPolling = () => {
        if (pollTimer !== undefined) { window.clearInterval(pollTimer); pollTimer = undefined; }
    };
    const onVisibility = () => {
        if (document.hidden) stopPolling();
        else { refresh(); startPolling(); }
    };

    onMounted(() => {
        refresh();
        startPolling();
        document.addEventListener('visibilitychange', onVisibility);
    });
    onActivated(() => {
        refresh();
        startPolling();
    });
    onDeactivated(() => stopPolling());
    onUnmounted(() => {
        stopPolling();
        document.removeEventListener('visibilitychange', onVisibility);
    });

    // -- Time + value formatting ---------------------------------------
    const parseSqlAt = (s: string | null | undefined): number => {
        if (!s) return 0;
        return new Date(s.replace(' ', 'T') + 'Z').getTime() || 0;
    };

    const formatRelative = (ms: number): string => {
        if (!ms) return '';
        const diff = Date.now() - ms;
        const s = Math.round(diff / 1000);
        if (s < 60) return `${s}s ago`;
        const m = Math.round(s / 60);
        if (m < 60) return `${m} min ago`;
        const h = Math.round(m / 60);
        if (h < 48) return `${h}h ago`;
        return new Date(ms).toLocaleString();
    };

    const formatMs = (t: number | null): string => {
        if (t == null || t <= 0) return '-';
        const totalSec = Math.floor(t / 1000);
        const m = Math.floor(totalSec / 60);
        const s = totalSec % 60;
        const mmm = t % 1000;
        if (m > 0) return `${m}:${s.toString().padStart(2, '0')}.${mmm.toString().padStart(3, '0')}`;
        return `${s}.${mmm.toString().padStart(3, '0')}`;
    };

    const flagUrl = (country: string | null | undefined): string | null => {
        if (!country) return null;
        if (country === '_404' || country === 'XX') return null;
        return `https://defrag.racing/images/flags/${country.toLowerCase()}.png`;
    };

    // -- Filtering ----------------------------------------------------
    const recordList = computed<RecordNotificationRow[]>(() => feed.value?.records ?? []);
    const systemList = computed<SystemNotificationRow[]>(() => feed.value?.system ?? []);

    const visibleRecords = computed(() => {
        const all = recordList.value;
        if (recordTab.value === 'worldrecords') return all.filter((r) => r.worldrecord);
        if (recordTab.value === 'beaten') return all.filter((r) => !r.worldrecord);
        return all;
    });

    const clanTypes = ['clan_invite','clan_kick','clan_accept','clan_leave','clan_transfer','clan_request','clan_request_accept','clan_request_reject'];
    const tournamentTypes = ['tournament_start','round_start','round_end'];

    const visibleSystem = computed(() => {
        const all = systemList.value;
        switch (systemTab.value) {
            case 'announcements': return all.filter((n) => n.type === 'announcement');
            case 'clan':          return all.filter((n) => clanTypes.includes(n.type));
            case 'tournament':    return all.filter((n) => tournamentTypes.includes(n.type));
            case 'profile':       return all.filter((n) => n.type === 'alias_suggestion');
            case 'render':        return all.filter((n) => n.type === 'render_completed' || n.type === 'render_failed');
            default:              return all;
        }
    });

    // -- Counts (unread, per sub-tab) ---------------------------------
    const recordTabCounts = computed(() => {
        const unread = recordList.value.filter((r) => !r.read);
        return {
            all: unread.length,
            beaten: unread.filter((r) => !r.worldrecord).length,
            worldrecords: unread.filter((r) => r.worldrecord).length,
        };
    });

    const systemTabCounts = computed(() => {
        const unread = systemList.value.filter((n) => !n.read);
        return {
            all: unread.length,
            announcements: unread.filter((n) => n.type === 'announcement').length,
            clan: unread.filter((n) => clanTypes.includes(n.type)).length,
            tournament: unread.filter((n) => tournamentTypes.includes(n.type)).length,
            profile: unread.filter((n) => n.type === 'alias_suggestion').length,
            render: unread.filter((n) => n.type === 'render_completed' || n.type === 'render_failed').length,
        };
    });

    const unreadMain = computed(() => ({
        records: feed.value?.unread.records ?? 0,
        system: feed.value?.unread.system ?? 0,
    }));

    // -- System notification type meta --------------------------------
    type TypeInfo = { label: string; icon: string; tone: string };
    const SYSTEM_TYPES: Record<string, TypeInfo> = {
        announcement:          { label: 'Announcement', icon: '📢', tone: 'blue' },
        clan_invite:           { label: 'Clan invite',  icon: '🛡️', tone: 'green' },
        clan_kick:             { label: 'Clan kick',    icon: '🛡️', tone: 'red' },
        clan_accept:           { label: 'Clan accept',  icon: '🛡️', tone: 'green' },
        clan_leave:            { label: 'Clan leave',   icon: '🛡️', tone: 'orange' },
        clan_transfer:         { label: 'Clan transfer', icon: '🛡️', tone: 'amber' },
        clan_request:          { label: 'Clan request', icon: '🛡️', tone: 'teal' },
        clan_request_accept:   { label: 'Clan request accepted', icon: '🛡️', tone: 'green' },
        clan_request_reject:   { label: 'Clan request declined', icon: '🛡️', tone: 'red' },
        tournament_start:      { label: 'Tournament start', icon: '🏆', tone: 'pink' },
        round_start:           { label: 'Round start',  icon: '▶️', tone: 'cyan' },
        round_end:             { label: 'Round end',    icon: '⏹️', tone: 'indigo' },
        alias_suggestion:      { label: 'Alias suggestion', icon: '🆔', tone: 'indigo' },
        render_completed:      { label: 'Render done',  icon: '📺', tone: 'emerald' },
        render_failed:         { label: 'Render failed', icon: '⚠️', tone: 'red' },
    };
    const typeInfoOf = (n: SystemNotificationRow): TypeInfo =>
        SYSTEM_TYPES[n.type] ?? { label: n.type || 'Notification', icon: '•', tone: 'neutral' };

    /** Tailwind classes per tone. Tailwind's JIT scrapes statics so we
     *  use a switch + concrete strings rather than template literals. */
    const toneClasses = (tone: string): { bg: string; border: string; text: string } => {
        switch (tone) {
            case 'blue':    return { bg: 'bg-blue-500/20',    border: 'border-blue-500/30',    text: 'text-blue-300' };
            case 'green':   return { bg: 'bg-green-500/20',   border: 'border-green-500/30',   text: 'text-green-300' };
            case 'red':     return { bg: 'bg-red-500/20',     border: 'border-red-500/30',     text: 'text-red-300' };
            case 'orange':  return { bg: 'bg-orange-500/20',  border: 'border-orange-500/30',  text: 'text-orange-300' };
            case 'amber':   return { bg: 'bg-amber-500/20',   border: 'border-amber-500/30',   text: 'text-amber-300' };
            case 'teal':    return { bg: 'bg-teal-500/20',    border: 'border-teal-500/30',    text: 'text-teal-300' };
            case 'pink':    return { bg: 'bg-pink-500/20',    border: 'border-pink-500/30',    text: 'text-pink-300' };
            case 'cyan':    return { bg: 'bg-cyan-500/20',    border: 'border-cyan-500/30',    text: 'text-cyan-300' };
            case 'indigo':  return { bg: 'bg-indigo-500/20',  border: 'border-indigo-500/30',  text: 'text-indigo-300' };
            case 'emerald': return { bg: 'bg-emerald-500/20', border: 'border-emerald-500/30', text: 'text-emerald-300' };
            default:        return { bg: 'bg-white/5',        border: 'border-white/10',       text: 'text-neutral-300' };
        }
    };

    // -- Click actions ------------------------------------------------
    const openSystemLink = (n: SystemNotificationRow) => {
        if (!n.url) return;
        openUrl(n.url).catch(() => {});
    };

    const openMap = (mapname: string | null) => {
        if (!mapname) return;
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`).catch(() => {});
    };

    const openProfile = (mddId: number | null) => {
        if (!mddId) return;
        openUrl(`https://defrag.racing/profile/${mddId}`).catch(() => {});
    };

    const physicsTone = (physics: string | null): string => {
        if (!physics) return 'neutral';
        return physics.toLowerCase().includes('cpm') ? 'pink' : 'blue';
    };

    const prefixFor = (type: string | null | undefined): string | null => {
        if (!type) return null;
        if (type.startsWith('clan_')) return 'Clan';
        if (type.startsWith('tournament_') || type.startsWith('round_')) return 'Tournament';
        if (type === 'alias_suggestion') return 'Alias';
        if (type === 'announcement') return 'Announcement';
        if (type === 'render_completed' || type === 'render_failed') return 'Render';
        return null;
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <!-- Header -->
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Notifications</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    <span v-if="(unreadMain.records + unreadMain.system) > 0">
                        {{ unreadMain.records + unreadMain.system }} unread
                    </span>
                    <span v-else>All caught up.</span>
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs flex-shrink-0">
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="refresh"
                >{{ loading ? 'Loading…' : 'Refresh' }}</button>
            </div>
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    Notifications need a token from your defrag.racing account.
                    Open Settings to paste one.
                </p>
            </div>
        </div>

        <template v-else>
            <!-- Main tabs (Records / System) -->
            <div class="flex border-b border-white/10">
                <button
                    class="flex-1 px-4 py-2 text-sm font-semibold transition-all relative flex items-center justify-center gap-2"
                    :class="mainTab === 'records' ? 'text-orange-400 bg-orange-500/10' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                    @click="mainTab = 'records'"
                >
                    <span>🏁</span>
                    <span>Record notifications</span>
                    <span
                        v-if="unreadMain.records > 0"
                        class="px-2 py-0.5 rounded-full text-[10px] font-bold"
                        :class="mainTab === 'records' ? 'bg-orange-500/30 text-orange-200' : 'bg-white/10 text-neutral-300'"
                    >{{ unreadMain.records }}</span>
                </button>
                <button
                    class="flex-1 px-4 py-2 text-sm font-semibold transition-all relative flex items-center justify-center gap-2"
                    :class="mainTab === 'system' ? 'text-blue-400 bg-blue-500/10' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                    @click="mainTab = 'system'"
                >
                    <span>📢</span>
                    <span>System notifications</span>
                    <span
                        v-if="unreadMain.system > 0"
                        class="px-2 py-0.5 rounded-full text-[10px] font-bold"
                        :class="mainTab === 'system' ? 'bg-blue-500/30 text-blue-200' : 'bg-white/10 text-neutral-300'"
                    >{{ unreadMain.system }}</span>
                </button>
            </div>

            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <!-- Records tab -->
            <template v-if="mainTab === 'records'">
                <!-- Records sub-tabs -->
                <div class="flex border-b border-white/[0.04] bg-black/20 text-xs">
                    <button
                        v-for="opt in ([
                            { v: 'all',          label: 'All records',     tone: 'orange' },
                            { v: 'beaten',       label: 'Beaten by others', tone: 'blue' },
                            { v: 'worldrecords', label: 'World records taken', tone: 'yellow' },
                        ] as const)"
                        :key="opt.v"
                        class="flex-1 px-3 py-1.5 transition-colors flex items-center justify-center gap-1.5"
                        :class="recordTab === opt.v
                            ? (opt.tone === 'orange' ? 'text-orange-300 bg-orange-500/10 font-semibold'
                              : opt.tone === 'blue' ? 'text-blue-300 bg-blue-500/10 font-semibold'
                              : 'text-yellow-300 bg-yellow-500/10 font-semibold')
                            : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                        @click="recordTab = opt.v"
                    >
                        <span>{{ opt.label }}</span>
                        <span
                            v-if="recordTabCounts[opt.v]"
                            class="px-1.5 py-0.5 rounded-full text-[10px] font-bold bg-white/10 text-neutral-200"
                        >{{ recordTabCounts[opt.v] }}</span>
                    </button>
                </div>

                <div class="flex-1 overflow-auto">
                    <div v-if="loading && !feed" class="p-8 text-center text-sm text-neutral-500">Loading…</div>
                    <div v-else-if="!visibleRecords.length" class="h-full flex items-center justify-center p-8">
                        <div class="text-center space-y-2 max-w-sm">
                            <div class="text-5xl">🏁</div>
                            <div class="text-neutral-300 font-semibold">No record notifications</div>
                            <p class="text-sm text-neutral-500">
                                When someone beats your PB or takes a WR you hold, it'll appear here.
                            </p>
                        </div>
                    </div>

                    <ul v-else class="divide-y divide-white/[0.04]">
                        <li
                            v-for="n in visibleRecords"
                            :key="n.id"
                            class="px-4 py-2.5 flex items-center gap-3 transition-colors"
                            :class="!n.read
                                ? (n.worldrecord ? 'bg-yellow-500/[0.06]' : 'bg-orange-500/[0.04]')
                                : 'opacity-60 hover:opacity-100'"
                        >
                            <!-- Icon -->
                            <div
                                class="w-7 h-7 rounded border flex items-center justify-center text-sm flex-shrink-0"
                                :class="n.worldrecord
                                    ? 'bg-yellow-500/20 border-yellow-500/30 text-yellow-300'
                                    : 'bg-orange-500/20 border-orange-500/30 text-orange-300'"
                                :title="n.worldrecord ? 'World record taken' : 'Personal best beaten'"
                            >{{ n.worldrecord ? '🏆' : '🏁' }}</div>

                            <!-- Content -->
                            <div class="flex-1 min-w-0 text-sm text-neutral-300 flex items-center gap-1.5 flex-wrap">
                                <img
                                    v-if="flagUrl(n.country)"
                                    :src="flagUrl(n.country)!"
                                    :alt="n.country || ''"
                                    class="w-4 h-3 rounded-sm flex-shrink-0"
                                    @error="($event.target as HTMLImageElement).style.display='none'"
                                />
                                <button
                                    class="font-bold text-neutral-100 hover:text-brand-300 hover:underline"
                                    @click.stop="openProfile(n.mdd_id)"
                                    v-html="q3ToHtml(n.name || 'Someone')"
                                ></button>
                                <span
                                    v-if="n.physics"
                                    class="uppercase text-[10px] px-1.5 py-0.5 rounded font-bold"
                                    :class="physicsTone(n.physics) === 'pink'
                                        ? 'bg-pink-500/20 text-pink-300'
                                        : 'bg-blue-500/20 text-blue-300'"
                                >{{ n.physics }}</span>
                                <span class="text-neutral-500">
                                    {{ n.worldrecord ? 'took the world record on' : 'broke your time on' }}
                                </span>
                                <button
                                    v-if="n.mapname"
                                    class="text-brand-400 hover:text-brand-300 font-semibold hover:underline"
                                    @click.stop="openMap(n.mapname)"
                                >{{ n.mapname }}</button>
                                <span class="text-neutral-500">with</span>
                                <span class="font-mono font-bold text-emerald-300">{{ formatMs(n.time) }}</span>
                                <span
                                    v-if="n.my_time && n.time && n.my_time > n.time"
                                    class="text-emerald-400 font-mono"
                                >(-{{ formatMs(n.my_time - n.time) }})</span>
                            </div>

                            <!-- Timestamp -->
                            <div class="text-[11px] text-neutral-500 flex-shrink-0 whitespace-nowrap">
                                {{ formatRelative(parseSqlAt(n.date_set ?? n.created_at)) }}
                            </div>
                        </li>
                    </ul>
                </div>
            </template>

            <!-- System tab -->
            <template v-else>
                <!-- System sub-tabs -->
                <div class="flex border-b border-white/[0.04] bg-black/20 text-xs overflow-x-auto">
                    <button
                        v-for="opt in ([
                            { v: 'all',           label: 'All' },
                            { v: 'announcements', label: 'Announcements' },
                            { v: 'clan',          label: 'Clan' },
                            { v: 'tournament',    label: 'Tournament' },
                            { v: 'profile',       label: 'Profile' },
                            { v: 'render',        label: 'Render' },
                        ] as const)"
                        :key="opt.v"
                        class="flex-1 min-w-[80px] px-3 py-1.5 transition-colors flex items-center justify-center gap-1.5 whitespace-nowrap"
                        :class="systemTab === opt.v
                            ? 'text-blue-300 bg-blue-500/10 font-semibold'
                            : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                        @click="systemTab = opt.v"
                    >
                        <span>{{ opt.label }}</span>
                        <span
                            v-if="systemTabCounts[opt.v]"
                            class="px-1.5 py-0.5 rounded-full text-[10px] font-bold bg-white/10 text-neutral-200"
                        >{{ systemTabCounts[opt.v] }}</span>
                    </button>
                </div>

                <div class="flex-1 overflow-auto">
                    <div v-if="loading && !feed" class="p-8 text-center text-sm text-neutral-500">Loading…</div>
                    <div v-else-if="!visibleSystem.length" class="h-full flex items-center justify-center p-8">
                        <div class="text-center space-y-2 max-w-sm">
                            <div class="text-5xl">📭</div>
                            <div class="text-neutral-300 font-semibold">No notifications here</div>
                            <p class="text-sm text-neutral-500">
                                Renders, clan events and announcements for your account will appear in this list.
                            </p>
                        </div>
                    </div>

                    <ul v-else class="divide-y divide-white/[0.04]">
                        <li
                            v-for="n in visibleSystem"
                            :key="n.id"
                            class="px-4 py-2.5 flex items-start gap-3 transition-colors"
                            :class="!n.read ? 'bg-blue-500/[0.04]' : 'opacity-60 hover:opacity-100'"
                        >
                            <!-- Type icon -->
                            <div
                                class="w-7 h-7 rounded border flex items-center justify-center text-sm flex-shrink-0"
                                :class="[toneClasses(typeInfoOf(n).tone).bg, toneClasses(typeInfoOf(n).tone).border, toneClasses(typeInfoOf(n).tone).text]"
                                :title="typeInfoOf(n).label"
                            >{{ typeInfoOf(n).icon }}</div>

                            <!-- Body -->
                            <div class="flex-1 min-w-0 text-sm text-neutral-300">
                                <div class="flex items-center gap-1.5 flex-wrap">
                                    <span
                                        v-if="prefixFor(n.type)"
                                        class="text-[10px] uppercase font-semibold px-1.5 py-0.5 rounded"
                                        :class="[toneClasses(typeInfoOf(n).tone).bg, toneClasses(typeInfoOf(n).tone).text]"
                                    >{{ prefixFor(n.type) }}</span>
                                    <span v-if="n.before" class="text-neutral-400" v-html="q3ToHtml(n.before)"></span>
                                    <button
                                        v-if="n.headline && n.url"
                                        class="font-bold text-brand-300 hover:underline"
                                        @click.stop="openSystemLink(n)"
                                        v-html="q3ToHtml(n.headline)"
                                    ></button>
                                    <strong
                                        v-else-if="n.headline"
                                        class="text-neutral-100"
                                        v-html="q3ToHtml(n.headline)"
                                    ></strong>
                                    <span v-if="n.after" class="text-neutral-400" v-html="q3ToHtml(n.after)"></span>
                                </div>
                                <div v-if="n.subheadline && n.type !== 'render_completed'" class="text-xs text-neutral-500 mt-0.5">
                                    {{ n.subheadline }}
                                </div>
                                <!-- Render-completed extras: a YouTube CTA so the user can jump straight to the rendered video. -->
                                <div v-if="n.type === 'render_completed'" class="flex items-center gap-2 mt-1">
                                    <button
                                        v-if="n.subheadline"
                                        class="text-[11px] inline-flex items-center gap-1 px-2 py-0.5 rounded bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-200"
                                        @click.stop="openUrl(n.subheadline!).catch(() => {})"
                                    >📥 Demo</button>
                                    <button
                                        v-if="n.url"
                                        class="text-[11px] inline-flex items-center gap-1 px-2 py-0.5 rounded bg-red-500/20 hover:bg-red-500/30 text-red-200"
                                        @click.stop="openSystemLink(n)"
                                    >▶ Watch on YouTube</button>
                                </div>
                                <div v-else-if="n.type === 'alias_suggestion' && n.url" class="mt-1">
                                    <button
                                        class="text-[11px] px-2 py-0.5 rounded bg-yellow-500/20 hover:bg-yellow-500/30 text-yellow-200 inline-flex items-center gap-1"
                                        @click.stop="openSystemLink(n)"
                                    >✓ Approve / reject</button>
                                </div>
                            </div>

                            <!-- Timestamp -->
                            <div class="text-[11px] text-neutral-500 flex-shrink-0 whitespace-nowrap">
                                {{ formatRelative(parseSqlAt(n.created_at)) }}
                            </div>
                        </li>
                    </ul>
                </div>
            </template>
        </template>
    </div>
</template>
