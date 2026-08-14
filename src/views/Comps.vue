<script setup lang="ts">
    // Comps: the weekly competition, from the launcher.
    //
    // Shows what is being played, what the user themselves has entered, and
    // which demos are waiting on an answer. It deliberately shows no other
    // competitor's time - not even a count of who is ahead - because times
    // stay hidden while a round is running and a launcher that leaked one
    // would be the way around the rule the site enforces. Entrant counts only.
    //
    // Voting lives on the website. The ballot has a preview video per map and
    // the launcher cannot play it, so voting here would mean voting blind; the
    // panel lists the five names and opens the site.

    import { computed, onActivated, onMounted, onUnmounted, ref } from 'vue';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { tauri, type CompsNotice, type CompsPayload, type PendingUpload, type UploadStateSnapshot } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openExternal } from '../lib/open';

    const config = useConfigStore();

    const data = ref<CompsPayload | null>(null);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const queue = ref<UploadStateSnapshot | null>(null);
    const busy = ref<Set<string>>(new Set());
    const actionError = ref<string | null>(null);
    const launchNote = ref<string | null>(null);

    // Ticks every second so the countdown moves without a network call.
    const now = ref(Date.now());
    let clockTimer: number | undefined;
    let pollTimer: number | undefined;
    let unlisten: UnlistenFn | null = null;

    const load = async (force = false) => {
        if (!config.hasToken) return;
        loading.value = true;
        error.value = null;
        try {
            data.value = force ? await tauri.refreshComps() : await tauri.getComps();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Could not load comps';
        } finally {
            loading.value = false;
        }
    };

    const loadQueue = async () => {
        try { queue.value = await tauri.getUploadState(); } catch { /* the panel just stays empty */ }
    };

    const playing = computed(() => data.value?.playing ?? null);
    const voting = computed(() => data.value?.voting ?? null);

    /** Demos the guard is holding: the whole point of the tab for anyone who
     *  just finished a run. */
    const held = computed<PendingUpload[]>(() =>
        (queue.value?.items ?? []).filter((i) => i.status === 'held_for_comps'),
    );

    const entered = computed<PendingUpload[]>(() =>
        (queue.value?.items ?? []).filter((i) => i.status === 'comps_entered'),
    );

    /** Demos the SERVER is holding, as opposed to `held` above, which is this
     *  machine's queue. The two answer different questions: `held` is "what is
     *  waiting for me to press a button", this is "what did the site do with
     *  the ones I already sent". A demo can be here having been uploaded from
     *  another machine, or months ago. */
    const notices = computed<CompsNotice[]>(() => data.value?.my_notices ?? []);

    const appearsAt = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : '');

    const physicsOrder = ['cpm', 'vq3'];
    const mapRows = computed(() => {
        const maps = playing.value?.maps ?? {};
        return physicsOrder
            .filter((p) => maps[p])
            .map((p) => ({
                physics: p,
                map: maps[p] as string,
                entrants: playing.value?.entrants?.[p] ?? 0,
            }));
    });

    const endsIn = computed(() => {
        const ends = playing.value?.ends_at;
        if (!ends) return null;
        const ms = new Date(ends).getTime() - now.value;
        if (Number.isNaN(ms)) return null;
        if (ms <= 0) return 'ending now';
        const d = Math.floor(ms / 86_400_000);
        const h = Math.floor((ms % 86_400_000) / 3_600_000);
        const m = Math.floor((ms % 3_600_000) / 60_000);
        if (d > 0) return `${d}d ${h}h`;
        if (h > 0) return `${h}h ${m}m`;
        return `${m}m`;
    });

    const endsAtLocal = computed(() => {
        const ends = playing.value?.ends_at;
        return ends ? new Date(ends).toLocaleString() : '';
    });

    const closesAtLocal = computed(() => {
        const closes = voting.value?.closes_at;
        return closes ? new Date(closes).toLocaleString() : '';
    });

    const fetchedAtLabel = computed(() => {
        const ms = data.value?.fetched_at_ms;
        if (!ms) return '';
        const sec = Math.round((now.value - ms) / 1000);
        if (sec < 60) return 'just now';
        const m = Math.floor(sec / 60);
        if (m < 60) return `${m}m ago`;
        return new Date(ms).toLocaleString();
    });

    const entryStatus = (status: string) => {
        if (status === 'valid') return { label: 'Counted', color: 'text-emerald-400' };
        if (status === 'invalid') return { label: 'Not counted', color: 'text-red-400' };
        return { label: 'Being checked', color: 'text-amber-300' };
    };

    const guardModeNote = computed(() => {
        const mode = config.config.comps_mode ?? 'ask';
        if (mode === 'auto') return 'Runs on these maps are entered into comps automatically.';
        if (mode === 'off') return 'The guard is off: runs on these maps are backed up publicly like any other demo.';
        return 'Runs on these maps are held back and you choose what happens to them.';
    });

    const answer = async (item: PendingUpload, enter: boolean) => {
        actionError.value = null;
        busy.value = new Set(busy.value).add(item.path);
        try {
            if (enter) await tauri.compsEnter(item.path);
            else await tauri.compsUploadNormally(item.path);
            void tauri.compsMarkIntroSeen();
            // The worker answers on its own schedule; the row updates through
            // the upload_state_changed listener.
        } catch (e: any) {
            actionError.value = e?.toString?.() ?? 'Could not send the demo';
        } finally {
            const next = new Set(busy.value);
            next.delete(item.path);
            busy.value = next;
        }
    };

    const playMap = async (map: string, physics: string) => {
        launchNote.value = null;
        try {
            const res = await tauri.runMapOffline(map, physics as 'vq3' | 'cpm', `${map}.pk3`);
            if (res.downloaded) launchNote.value = `Downloaded ${map}.pk3 and started the game.`;
        } catch (e: any) {
            // The pk3 is guessed from the map name, which holds for most maps
            // but not all - say so instead of leaving a dead button.
            launchNote.value =
                `Could not start ${map}: ${e?.toString?.() ?? 'unknown error'}. ` +
                'Install it from the Maps tab and try again.';
        }
    };

    const openMap = (map: string) =>
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(map)}`).catch(() => {});
    const openComps = () => openExternal('https://defrag.racing/comps').catch(() => {});

    onMounted(async () => {
        void load();
        void loadQueue();
        clockTimer = window.setInterval(() => { now.value = Date.now(); }, 1000);
        pollTimer = window.setInterval(() => { if (!document.hidden) void load(); }, 60_000);
        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            queue.value = ev.payload;
        });
    });
    onActivated(() => {
        void load();
        void loadQueue();
    });
    onUnmounted(() => {
        if (clockTimer !== undefined) window.clearInterval(clockTimer);
        if (pollTimer !== undefined) window.clearInterval(pollTimer);
        unlisten?.();
    });
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Comps</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    The weekly competition. Nobody's time is shown while a round is running - not even yours to them.
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs text-neutral-500 flex-shrink-0">
                <span v-if="data?.stale" class="text-amber-300/80" title="The last refresh failed - this is the last copy the launcher managed to fetch.">
                    Offline copy from {{ fetchedAtLabel }}
                </span>
                <span v-else-if="data">Updated {{ fetchedAtLabel }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="load(true)"
                >{{ loading ? 'Loading…' : 'Refresh' }}</button>
            </div>
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    Comps needs a token from your defrag.racing account - it is how the launcher
                    knows which entries are yours.
                </p>
                <RouterLink
                    :to="{ name: 'settings', query: { highlight: 'token' } }"
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                >Open Settings to paste a token →</RouterLink>
            </div>
        </div>

        <template v-else>
            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>
            <p v-if="actionError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ actionError }}
            </p>
            <p v-if="launchNote" class="px-5 py-2 bg-white/5 border-b border-white/10 text-xs text-neutral-300">
                {{ launchNote }}
            </p>

            <div class="flex-1 overflow-auto p-3 space-y-3">
                <!-- Demos waiting on an answer. First, because someone who has
                     just finished a run opens this tab to deal with exactly
                     this and nothing else. -->
                <section v-if="held.length" class="bg-amber-500/[0.07] border border-amber-500/25 rounded-lg">
                    <header class="px-3 py-2 border-b border-amber-500/20 flex items-center gap-2">
                        <span class="text-sm font-semibold text-amber-200">
                            {{ held.length === 1 ? 'A demo is waiting for you' : `${held.length} demos are waiting for you` }}
                        </span>
                    </header>
                    <p class="px-3 pt-2 text-xs text-neutral-400">
                        These look like runs of a map being played this week, so the launcher did
                        <strong class="text-neutral-200">not</strong> back them up - a comps run published
                        mid-round cannot be taken back. Enter it, or upload it the normal way.
                    </p>
                    <ul class="p-3 space-y-2">
                        <li
                            v-for="item in held"
                            :key="item.path"
                            class="flex items-center justify-between gap-3 bg-black/20 border border-white/5 rounded px-3 py-2"
                        >
                            <div class="min-w-0">
                                <div class="text-sm text-neutral-200 truncate" :title="item.path">{{ item.filename }}</div>
                                <div class="text-[11px] text-neutral-500">
                                    <span v-if="item.comps">
                                        matches <span class="text-neutral-300">{{ item.comps.map }}</span>
                                        ({{ item.comps.physics.toUpperCase() }})
                                    </span>
                                    <span v-if="item.error" class="text-red-400"> · {{ item.error }}</span>
                                </div>
                            </div>
                            <div class="flex items-center gap-2 flex-shrink-0">
                                <button
                                    class="px-2.5 py-1 rounded text-xs font-semibold bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40"
                                    :disabled="busy.has(item.path)"
                                    @click="answer(item, true)"
                                >Enter into comps</button>
                                <button
                                    class="px-2.5 py-1 rounded text-xs bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-40"
                                    :disabled="busy.has(item.path)"
                                    title="Upload it like any other demo. This decides this file only - the next one is asked about again."
                                    @click="answer(item, false)"
                                >Upload normally</button>
                            </div>
                        </li>
                    </ul>
                </section>

                <!-- What is being played -->
                <section v-if="playing" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between gap-3">
                        <div class="text-sm font-semibold text-neutral-200">
                            Weekly #{{ playing.comp_number }}
                            <span v-if="playing.category" class="text-neutral-500 font-normal">
                                · {{ playing.category }}<span v-if="playing.weapon"> ({{ playing.weapon }})</span>
                            </span>
                        </div>
                        <div class="text-xs text-neutral-400 flex items-center gap-3">
                            <span v-if="playing.prize_eur" class="text-emerald-300">
                                {{ playing.prize_eur.toFixed(2) }} € per physics
                            </span>
                            <span v-if="endsIn" :title="endsAtLocal">ends in {{ endsIn }}</span>
                        </div>
                    </header>

                    <div class="p-3 grid grid-cols-1 sm:grid-cols-2 gap-2">
                        <div
                            v-for="row in mapRows"
                            :key="row.physics"
                            class="bg-black/20 border border-white/5 rounded px-3 py-2 flex items-center justify-between gap-3"
                        >
                            <div class="min-w-0">
                                <div class="text-[10px] uppercase text-neutral-500">{{ row.physics }}</div>
                                <button
                                    class="text-sm text-brand-400 hover:underline truncate max-w-full text-left"
                                    :title="`Open ${row.map} on defrag.racing`"
                                    @click="openMap(row.map)"
                                >{{ row.map }}</button>
                                <div class="text-[11px] text-neutral-500">
                                    {{ row.entrants }} {{ row.entrants === 1 ? 'player entered' : 'players entered' }}
                                </div>
                            </div>
                            <button
                                class="px-2.5 py-1 rounded text-xs font-semibold bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 flex-shrink-0"
                                :disabled="!config.config.engine_path"
                                :title="config.config.engine_path ? `Play ${row.map} in ${row.physics.toUpperCase()}` : 'Pick an engine in Settings first'"
                                @click="playMap(row.map, row.physics)"
                            >Play</button>
                        </div>
                    </div>

                    <!-- The user's own entries. Their own times, which are
                         theirs to see; the invalid_reason arrives as a finished
                         sentence so the launcher needs to know nothing about
                         the rules to explain a refusal. -->
                    <div class="px-3 pb-3">
                        <div class="text-[10px] uppercase text-neutral-500 mb-1">Your entries</div>
                        <table v-if="playing.my_entries.length" class="w-full text-xs">
                            <tbody>
                                <tr
                                    v-for="e in playing.my_entries"
                                    :key="e.id"
                                    class="border-t border-white/[0.03]"
                                >
                                    <td class="py-1 pr-2 text-neutral-300 truncate max-w-[18rem]" :title="e.filename || ''">
                                        {{ e.filename || '(demo)' }}
                                    </td>
                                    <td class="py-1 pr-2 text-neutral-500 uppercase">{{ e.physics || '-' }}</td>
                                    <td class="py-1 pr-2 text-right font-mono text-emerald-300">{{ e.time || '-' }}</td>
                                    <td class="py-1 pr-2" :class="entryStatus(e.status).color">
                                        {{ entryStatus(e.status).label }}
                                    </td>
                                    <td class="py-1 text-neutral-500 truncate max-w-[16rem]" :title="e.invalid_reason || ''">
                                        {{ e.invalid_reason || '' }}
                                    </td>
                                </tr>
                            </tbody>
                        </table>
                        <p v-else class="text-xs text-neutral-500">
                            Nothing entered yet. Record a run on one of the maps above and the
                            launcher will offer to enter it.
                        </p>
                        <p v-if="entered.length" class="text-[11px] text-neutral-500 mt-2">
                            {{ entered.length }} {{ entered.length === 1 ? 'demo' : 'demos' }} sent from this
                            machine this session. Entries stay private until the round ends, then they are
                            published like any other demo.
                        </p>
                        <p class="text-[11px] text-neutral-500 mt-2">{{ guardModeNote }}</p>
                    </div>
                </section>

                <section v-else class="bg-neutral-900/40 border border-white/10 rounded-lg p-6 text-center">
                    <div class="text-sm text-neutral-300">No round is being played right now.</div>
                    <p class="text-xs text-neutral-500 mt-1">
                        A new weekly starts every Sunday at 20:00 Prague time.
                    </p>
                </section>

                <!-- What the site did with demos it decided not to enter.
                     Outside the round panel on purpose: a demo can be on hold
                     for a map that is still being voted on, which is a week
                     with no round being played at all. -->
                <section v-if="notices.length" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 text-sm font-semibold text-neutral-200">
                        Demos of yours on hold
                    </header>
                    <ul class="p-3 space-y-2.5">
                        <li v-for="n in notices" :key="n.id" class="text-xs">
                            <div class="truncate text-neutral-300" :title="n.filename || ''">
                                {{ n.filename || '(demo)' }}
                            </div>
                            <div :class="n.kind === 'unreadable' ? 'text-red-300' : 'text-neutral-500'">
                                {{ n.note }}
                            </div>
                            <div v-if="n.appears_at" class="text-[11px] text-neutral-600">
                                Appears {{ appearsAt(n.appears_at) }}
                            </div>
                            <!-- The one case the site cannot explain by itself:
                                 nobody here knows what is wrong with the file,
                                 so it hands over the person who can look. -->
                            <button
                                v-if="n.kind === 'unreadable'"
                                class="mt-1 text-[11px] text-brand-400 hover:underline"
                                @click="openComps"
                            >Tell the admin on defrag.racing →</button>
                        </li>
                    </ul>
                </section>

                <!-- The open ballot: names only. Voting happens on the site,
                     where the preview videos are. -->
                <section v-if="voting" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between gap-3">
                        <div class="text-sm font-semibold text-neutral-200">
                            Voting for weekly #{{ voting.comp_number }}
                            <span v-if="voting.category" class="text-neutral-500 font-normal">· {{ voting.category }}</span>
                        </div>
                        <div class="text-xs text-neutral-500">
                            <span v-if="voting.is_open" :title="closesAtLocal">closes {{ closesAtLocal }}</span>
                            <span v-else>closed</span>
                        </div>
                    </header>
                    <div class="p-3 flex flex-wrap gap-1.5">
                        <button
                            v-for="c in voting.candidates"
                            :key="c"
                            class="px-2 py-1 rounded bg-black/20 border border-white/5 text-xs text-brand-400 hover:underline"
                            @click="openMap(c)"
                        >{{ c }}</button>
                        <span v-if="!voting.candidates.length" class="text-xs text-neutral-500">
                            The ballot is not drawn yet.
                        </span>
                    </div>
                    <div class="px-3 pb-3">
                        <button
                            class="px-3 py-1.5 rounded text-xs font-semibold bg-brand-500/20 hover:bg-brand-500/30 text-brand-300"
                            @click="openComps"
                        >Vote on defrag.racing →</button>
                        <span class="text-[11px] text-neutral-500 ml-2">
                            Each map has a preview video the launcher cannot play, so voting happens on the site.
                        </span>
                    </div>
                </section>
            </div>
        </template>
    </div>
</template>
