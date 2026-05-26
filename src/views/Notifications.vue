<script setup lang="ts">
    // Notifications tab. Mirrors the web notifications center: two
    // feeds - record (PB beats, WR takes, alias suggestions) and
    // system (render completed, demome events). Both come from
    // /api/launcher/notifications which the web already serves.
    //
    // No mark-as-read locally - the existing web flow handles that
    // through a separate endpoint we haven't wired into the launcher
    // yet; for now the launcher is read-only on this feed. The user
    // can mark-read on the web and it'll reflect here on next poll.

    import { computed, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type NotificationsFeed, type SystemNotificationRow, type RecordNotificationRow } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const feed = ref<NotificationsFeed | null>(null);
    const loading = ref(true);
    const error = ref<string | null>(null);

    type Tab = 'all' | 'records' | 'system';
    const tab = ref<Tab>('all');

    const refresh = async () => {
        if (!config.hasToken) {
            loading.value = false;
            return;
        }
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

    onMounted(refresh);

    // Poll every 60s while the tab is open so a newly-completed
    // render shows up without the user clicking Refresh. Cheap
    // request (single SELECT + small payload) so the rate is fine.
    let pollTimer: number | undefined;
    onMounted(() => {
        pollTimer = window.setInterval(refresh, 60_000);
    });
    onUnmounted(() => {
        if (pollTimer !== undefined) window.clearInterval(pollTimer);
    });

    // Unified time-sort across both feeds so "All" reads as one
    // chronological list.
    type Entry =
        | { kind: 'record';  data: RecordNotificationRow;  at: number }
        | { kind: 'system';  data: SystemNotificationRow;  at: number };
    const allEntries = computed<Entry[]>(() => {
        if (!feed.value) return [];
        const records: Entry[] = feed.value.records.map((r) => ({
            kind: 'record',
            data: r,
            at: new Date(r.created_at.replace(' ', 'T') + 'Z').getTime() || 0,
        }));
        const system: Entry[] = feed.value.system.map((s) => ({
            kind: 'system',
            data: s,
            at: new Date(s.created_at.replace(' ', 'T') + 'Z').getTime() || 0,
        }));
        return [...records, ...system].sort((a, b) => b.at - a.at);
    });

    const visibleEntries = computed(() => {
        if (tab.value === 'records') return allEntries.value.filter((e) => e.kind === 'record');
        if (tab.value === 'system')  return allEntries.value.filter((e) => e.kind === 'system');
        return allEntries.value;
    });

    const formatTime = (ms: number): string => {
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

    const flagUrl = (country: string | null | undefined): string | null => {
        if (!country) return null;
        if (country === '_404' || country === 'XX') return null;
        return `https://defrag.racing/images/flags/${country.toLowerCase()}.png`;
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

    const openSystemLink = (n: SystemNotificationRow) => {
        if (!n.url) return;
        openUrl(n.url).catch(() => {});
    };

    const openMap = (mapname: string | null) => {
        if (!mapname) return;
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => {});
    };

    // Icon per system notification type. type comes from the backend
    // (DemomeController sets "render_completed", etc.); unknown types
    // get a neutral fallback so a new backend type doesn't render
    // blank.
    const iconForSystem = (n: SystemNotificationRow): string => {
        switch (n.type) {
            case 'render_completed': return '📺';
            case 'render_failed':    return '⚠️';
            case 'alias':            return '🆔';
            default:                 return '•';
        }
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Notifications</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    Records, renders, and other defrag.racing updates for your account.
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
            <div class="px-5 py-1.5 border-b border-white/[0.04] flex items-center gap-1 text-xs">
                <button
                    v-for="opt in ([
                        { v: 'all',     label: 'All' },
                        { v: 'records', label: 'Records' },
                        { v: 'system',  label: 'System' },
                    ] as const)"
                    :key="opt.v"
                    class="px-2.5 py-1 rounded transition-colors whitespace-nowrap"
                    :class="tab === opt.v ? 'bg-brand-500/25 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                    @click="tab = opt.v"
                >{{ opt.label }}
                    <span v-if="opt.v === 'records' && feed?.unread.records" class="ml-1 text-[10px] text-brand-300">({{ feed.unread.records }})</span>
                    <span v-if="opt.v === 'system' && feed?.unread.system" class="ml-1 text-[10px] text-brand-300">({{ feed.unread.system }})</span>
                </button>
            </div>

            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <div class="flex-1 overflow-auto">
                <div v-if="loading && !feed" class="p-8 text-center text-sm text-neutral-500">
                    Loading…
                </div>
                <div v-else-if="!visibleEntries.length" class="h-full flex items-center justify-center p-8">
                    <div class="text-center space-y-2 max-w-sm">
                        <div class="text-5xl">🔔</div>
                        <div class="text-neutral-300 font-semibold">Nothing here yet</div>
                        <p class="text-sm text-neutral-500">
                            When someone beats your PB or one of your demos finishes rendering, it'll show up here.
                        </p>
                    </div>
                </div>

                <ul v-else class="divide-y divide-white/[0.04]">
                    <li
                        v-for="entry in visibleEntries"
                        :key="`${entry.kind}-${entry.data.id}`"
                        class="px-5 py-3 flex items-start gap-3"
                        :class="!entry.data.read ? 'bg-brand-500/[0.04]' : ''"
                    >
                        <!-- Record notification: someone's PB / WR event. -->
                        <template v-if="entry.kind === 'record'">
                            <div class="text-xl flex-shrink-0">🏁</div>
                            <div class="flex-1 min-w-0 text-sm">
                                <div class="text-neutral-200 flex items-center gap-1.5 flex-wrap">
                                    <img
                                        v-if="flagUrl(entry.data.other_user_country)"
                                        :src="flagUrl(entry.data.other_user_country)!"
                                        :alt="entry.data.other_user_country || ''"
                                        class="w-3 h-2 rounded flex-shrink-0"
                                        @error="($event.target as HTMLImageElement).style.display='none'"
                                    />
                                    <span class="font-semibold">{{ entry.data.other_user_name || 'Someone' }}</span>
                                    <span class="text-neutral-500">set a record on</span>
                                    <button class="text-brand-400 hover:underline" @click="openMap(entry.data.map)">
                                        {{ entry.data.map }}
                                    </button>
                                    <span v-if="entry.data.physics" class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5 text-neutral-300">{{ entry.data.physics }}</span>
                                </div>
                                <div class="text-xs text-neutral-500 mt-0.5 flex items-center gap-2">
                                    <span class="text-emerald-300 font-mono">{{ formatMs(entry.data.time) }}</span>
                                    <span v-if="entry.data.rank">· rank {{ entry.data.rank }}</span>
                                    <span class="text-neutral-600">·</span>
                                    <span>{{ formatTime(entry.at) }}</span>
                                </div>
                            </div>
                        </template>

                        <!-- System notification: render done, alias, etc. -->
                        <template v-else>
                            <div class="text-xl flex-shrink-0">{{ iconForSystem(entry.data) }}</div>
                            <div class="flex-1 min-w-0 text-sm">
                                <div class="text-neutral-200 truncate">
                                    <span v-if="entry.data.before">{{ entry.data.before }}</span>
                                    <strong v-if="entry.data.headline" class="ml-1">{{ entry.data.headline }}</strong>
                                    <span v-if="entry.data.after" class="ml-1">{{ entry.data.after }}</span>
                                </div>
                                <div v-if="entry.data.subheadline" class="text-xs text-neutral-500 truncate mt-0.5">
                                    {{ entry.data.subheadline }}
                                </div>
                                <div class="text-xs text-neutral-500 mt-0.5 flex items-center gap-2">
                                    <button
                                        v-if="entry.data.url"
                                        class="text-brand-400 hover:underline"
                                        @click="openSystemLink(entry.data)"
                                    >Open →</button>
                                    <span v-if="entry.data.url" class="text-neutral-600">·</span>
                                    <span>{{ formatTime(entry.at) }}</span>
                                </div>
                            </div>
                        </template>
                    </li>
                </ul>
            </div>
        </template>
    </div>
</template>
