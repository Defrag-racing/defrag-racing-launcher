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
    import { useOnScreen } from '../lib/visibility';
    import { tauri, type CompsNotice, type CompsPayload, type PendingUpload, type UploadStateSnapshot } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import { openExternal } from '../lib/open';
    import { t } from '../lib/i18n';

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
            // Fill in any map the feed named without describing. Not awaited:
            // the panel is already worth showing, and the pictures arrive into
            // it as each answer comes back.
            fillInMaps();
        } catch (e: any) {
            error.value = e?.toString?.() ?? t('Could not load comps');
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

    /** Whether this account may enter at all. A server without the field does
     *  not enforce the rule, so silence means allowed. */
    const gate = computed(() => data.value?.entry_gate ?? null);

    const gateTitle = computed(() => {
        switch (gate.value?.needs) {
            case 'signin':
                return t('Sign in on defrag.racing');
            case 'verify':
                return t('Confirm your email address');
            default:
                return t('Link your Q3DF.org account');
        }
    });

    const openSettings = () =>
        openExternal(gate.value?.settings_url ?? 'https://defrag.racing/user/settings').catch(() => {});

    /** The pool behind the weeklies. Who donated stays on the site; this is the
     *  total, how far it reaches, and a way to add to it. */
    const pool = computed(() => data.value?.pool ?? null);

    const openDonations = () =>
        openExternal(pool.value?.donate_url ?? 'https://defrag.racing/donations').catch(() => {});

    const physicsOrder = ['cpm', 'vq3'];
    const mapRows = computed(() => {
        const maps = playing.value?.maps ?? {};
        const cards = playing.value?.map_cards ?? {};
        return physicsOrder
            .filter((p) => maps[p])
            .map((p) => ({
                physics: p,
                map: maps[p] as string,
                ...describe(maps[p] as string, cards[p]?.author, cards[p]?.thumbnail),
                entrants: playing.value?.entrants?.[p] ?? 0,
            }));
    });

    const endsIn = computed(() => {
        const ends = playing.value?.ends_at;
        if (!ends) return null;
        const ms = new Date(ends).getTime() - now.value;
        if (Number.isNaN(ms)) return null;
        if (ms <= 0) return t('ending now');
        const d = Math.floor(ms / 86_400_000);
        const h = Math.floor((ms % 86_400_000) / 3_600_000);
        const m = Math.floor((ms % 3_600_000) / 60_000);
        if (d > 0) return t(':days d :hours h', { days: d, hours: h });
        if (h > 0) return t(':hours h :minutes m', { hours: h, minutes: m });
        return t(':minutes m', { minutes: m });
    });

    const endsAtLocal = computed(() => {
        const ends = playing.value?.ends_at;
        return ends ? new Date(ends).toLocaleString() : '';
    });

    const closesAtLocal = computed(() => {
        const closes = voting.value?.closes_at;
        return closes ? new Date(closes).toLocaleString() : '';
    });

    /** What came out of the ballot, once it has closed. Empty while voting is
     *  open, and empty on a server too old to send it - in both cases the panel
     *  falls back to listing the candidates. */
    const decidedRows = computed(() => {
        const decided = voting.value?.decided ?? {};
        return physicsOrder
            .filter((p) => decided[p]?.map)
            .map((p) => ({
                physics: p,
                map: decided[p].map as string,
                ...describe(decided[p].map as string, decided[p].author, decided[p].thumbnail),
                votes: decided[p].votes ?? null,
                byWildcard: decided[p].decided_by === 'wildcard',
            }));
    });

    /** The ballot as pictures rather than a row of words. Falls back to the
     *  names a server too old to send the rest still provides. */
    const candidateCards = computed(() => {
        const cards = voting.value?.candidate_maps ?? [];
        if (cards.length) {
            return cards.map((c) => ({
                map: c.map ?? '',
                ...describe(c.map ?? '', c.author, c.thumbnail),
                // One line per physics, minus any this map is barred from -
                // a count under a physics it cannot win reads as a race it is
                // losing.
                votes: physicsOrder
                    .filter((p) => c.blocked_physics !== p)
                    .map((p) => ({ physics: p, count: c.votes?.[p] ?? 0 })),
            }));
        }
        // A site too old to send the ballot as cards still names the maps,
        // and the map list fills in the rest.
        return (voting.value?.candidates ?? []).map((map) => ({
            map,
            ...describe(map),
            votes: [] as { physics: string; count: number }[],
        }));
    });

    /** What the site puts where a map has no levelshot. A grey "no image" panel
     *  among four screenshots reads as a broken card rather than a map nobody
     *  has photographed. */
    const FALLBACK_THUMBNAIL = 'https://defrag.racing/images/unknown.jpg';

    const onThumbnailError = (e: Event) => {
        const img = e.target as HTMLImageElement;
        if (img.src !== FALLBACK_THUMBNAIL) img.src = FALLBACK_THUMBNAIL;
    };

    /** The site stores a `/storage`-relative path or an absolute URL. Same rule
     *  as the server browser, so both read one way. */
    const thumbnailUrl = (t: string | null | undefined): string | null => {
        if (!t) return null;
        if (t.startsWith('http://') || t.startsWith('https://')) return t;
        return `https://defrag.racing/storage/${t}`;
    };

    // ---- maps looked up by name -------------------------------------
    // The comps feed only started carrying pictures and authors in August
    // 2026, and a launcher is not much use waiting for a site to be updated -
    // the map list has had both all along and is one request away. Anything
    // the payload does not describe is looked up by name and remembered for
    // the session, so a tab that is refreshed every minute asks once.
    const lookedUp = ref<Record<string, { author: string | null; thumbnail: string | null }>>({});
    const lookingUp = new Set<string>();

    const lookUpMap = async (name: string) => {
        const key = name.toLowerCase();
        if (!name || lookingUp.has(key) || lookedUp.value[key]) return;
        lookingUp.add(key);
        try {
            // The search is a substring match, so the exact name has to be
            // picked back out of the page it comes in.
            const page = await tauri.getMaps(1, name);
            const hit = (page?.data ?? []).find((m) => m.name?.toLowerCase() === key);
            if (hit) {
                lookedUp.value = {
                    ...lookedUp.value,
                    [key]: { author: hit.author ?? null, thumbnail: hit.thumbnail ?? null },
                };
            }
        } catch {
            /* leave it undescribed - the row still shows the name */
        } finally {
            lookingUp.delete(key);
        }
    };

    /** Whatever the payload knows, filled in from the map list where it does
     *  not. */
    const describe = (name: string, author?: string | null, thumbnail?: string | null) => {
        const found = lookedUp.value[name?.toLowerCase() ?? ''];
        return {
            author: author ?? found?.author ?? null,
            thumbnail: thumbnailUrl(thumbnail ?? found?.thumbnail),
        };
    };

    /** Every map named anywhere in the payload, so the ones the site did not
     *  describe can be filled in. */
    const fillInMaps = () => {
        const names: string[] = [];
        const playingMaps = playing.value?.maps ?? {};
        const cards = playing.value?.map_cards ?? {};
        for (const p of physicsOrder) {
            if (playingMaps[p] && !cards[p]?.thumbnail) names.push(playingMaps[p] as string);
        }
        const decided = voting.value?.decided ?? {};
        for (const p of physicsOrder) {
            if (decided[p]?.map && !decided[p]?.thumbnail) names.push(decided[p].map as string);
        }
        const ballot = voting.value?.candidate_maps ?? [];
        if (ballot.length) {
            for (const c of ballot) if (c.map && !c.thumbnail) names.push(c.map);
        } else {
            names.push(...(voting.value?.candidates ?? []));
        }
        for (const n of names) void lookUpMap(n);
    };

    const startsAtLocal = computed(() => {
        const starts = voting.value?.starts_at;
        return starts ? new Date(starts).toLocaleString() : '';
    });

    /** How long until the next week begins. The same shape as the countdown on
     *  the round being played, so the two read alike. */
    const startsIn = computed(() => {
        const starts = voting.value?.starts_at;
        if (!starts) return null;
        const ms = new Date(starts).getTime() - now.value;
        if (Number.isNaN(ms) || ms <= 0) return null;
        const d = Math.floor(ms / 86_400_000);
        const h = Math.floor((ms % 86_400_000) / 3_600_000);
        const m = Math.floor((ms % 3_600_000) / 60_000);
        if (d > 0) return t(':days d :hours h', { days: d, hours: h });
        if (h > 0) return t(':hours h :minutes m', { hours: h, minutes: m });
        return t(':minutes m', { minutes: m });
    });

    const fetchedAtLabel = computed(() => {
        const ms = data.value?.fetched_at_ms;
        if (!ms) return '';
        const sec = Math.round((now.value - ms) / 1000);
        if (sec < 60) return t('just now');
        const m = Math.floor(sec / 60);
        if (m < 60) return t(':count minutes ago', { count: m });
        return new Date(ms).toLocaleString();
    });

    const entryStatus = (status: string) => {
        if (status === 'valid') return { label: t('Counted'), color: 'text-emerald-400' };
        if (status === 'invalid') return { label: t('Not counted'), color: 'text-red-400' };
        return { label: t('Being checked'), color: 'text-amber-300' };
    };

    const guardModeNote = computed(() => {
        const mode = config.config.comps_mode ?? 'ask';
        if (mode === 'auto') return t('Runs on these maps are entered into comps automatically.');
        if (mode === 'off') return t('The guard is off: runs on these maps are backed up publicly like any other demo.');
        return t('Runs on these maps are held back and you choose what happens to them.');
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
            actionError.value = e?.toString?.() ?? t('Could not send the demo');
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
            if (res.downloaded) launchNote.value = t('Downloaded :file and started the game.', { file: `${map}.pk3` });
        } catch (e: any) {
            // The pk3 is guessed from the map name, which holds for most maps
            // but not all - say so instead of leaving a dead button.
            launchNote.value = t('Could not start :map: :reason. Install it from the Maps tab and try again.', {
                map,
                reason: e?.toString?.() ?? t('unknown error'),
            });
        }
    };

    const openMap = (map: string) =>
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(map)}`).catch(() => {});
    const onScreen = useOnScreen();

    const openComps = () => openExternal('https://defrag.racing/comps').catch(() => {});

    onMounted(async () => {
        void load();
        void loadQueue();

        clockTimer = window.setInterval(() => { now.value = Date.now(); }, 1000);
        pollTimer = window.setInterval(() => { if (onScreen.value) void load(); }, 60_000);
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
                <div class="font-semibold">{{ $t('Comps') }}</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    {{ $t("The weekly competition. Nobody's time is shown while a round is running - not even yours to them.") }}
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs text-neutral-500 flex-shrink-0">
                <span v-if="data?.stale" class="text-amber-300/80" :title="$t('The last refresh failed - this is the last copy the launcher managed to fetch.')">
                    {{ $t('Offline copy from') }} {{ fetchedAtLabel }}
                </span>
                <span v-else-if="data">{{ $t('Updated') }} {{ fetchedAtLabel }}</span>
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading || !config.hasToken"
                    @click="load(true)"
                >{{ loading ? $t('Loading…') : $t('Refresh') }}</button>
            </div>
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">{{ $t('Token required') }}</div>
                <p class="text-sm text-neutral-500">
                    {{ $t('Comps needs a token from your defrag.racing account - it is how the launcher knows which entries are yours.') }}
                </p>
                <RouterLink
                    :to="{ name: 'settings', query: { highlight: 'token' } }"
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                >{{ $t('Open Settings to paste a token →') }}</RouterLink>
            </div>
        </div>

        <!-- Signed in, but not allowed to enter a run. The whole tab is that
             one sentence: a round they cannot be in is not worth a map list, a
             countdown or an entry table, and every button in here would lead
             to a refusal. Everything else in the launcher works as usual - a
             token is handed to any account, because backing demos up has
             nothing to do with comps. -->
        <div v-else-if="gate && !gate.may" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔗</div>
                <div class="text-neutral-300 font-semibold">{{ gateTitle }}</div>
                <p class="text-sm text-neutral-500">{{ gate.reason }}</p>
                <p class="text-xs text-neutral-600">
                    {{ $t('Everything else in the launcher keeps working - this is only about entering comps.') }}
                </p>
                <button
                    class="inline-flex items-center gap-1 mt-1 px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-sm font-semibold"
                    @click="openSettings"
                >{{ $t('Open defrag.racing settings →') }}</button>
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
                            {{ held.length === 1 ? $t('A demo is waiting for you') : $t(':count demos are waiting for you', { count: held.length }) }}
                        </span>
                    </header>
                    <p class="px-3 pt-2 text-xs text-neutral-400">
                        {{ $t('These look like runs of a map being played this week, so the launcher did NOT back them up - a comps run published mid-round cannot be taken back. Enter it, or upload it the normal way.') }}
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
                                        {{ $t('matches') }} <span class="text-neutral-300">{{ item.comps.map }}</span>
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
                                >{{ $t('Enter into comps') }}</button>
                                <button
                                    class="px-2.5 py-1 rounded text-xs bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-40"
                                    :disabled="busy.has(item.path)"
                                    :title="$t('Upload it like any other demo. This decides this file only - the next one is asked about again.')"
                                    @click="answer(item, false)"
                                >{{ $t('Upload normally') }}</button>
                            </div>
                        </li>
                    </ul>
                </section>

                <!-- What is being played -->
                <section v-if="playing" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between gap-3">
                        <div class="text-sm font-semibold text-neutral-200">
                            {{ $t('Weekly #:number', { number: playing.comp_number }) }}
                            <span v-if="playing.category" class="text-neutral-500 font-normal">
                                · {{ playing.category }}<span v-if="playing.weapon"> ({{ playing.weapon }})</span>
                            </span>
                        </div>
                        <div class="text-xs text-neutral-400 flex items-center gap-3">
                            <span v-if="playing.prize_eur" class="text-emerald-300">
                                {{ $t(':amount € per physics', { amount: playing.prize_eur.toFixed(2) }) }}
                            </span>
                            <span v-if="endsIn" :title="endsAtLocal">{{ $t('ends in :time', { time: endsIn }) }}</span>
                        </div>
                    </header>

                    <div class="p-3 grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div
                            v-for="row in mapRows"
                            :key="row.physics"
                            class="bg-black/25 border border-white/5 rounded-lg overflow-hidden flex items-stretch gap-3"
                        >
                            <button
                                class="m-2.5 w-32 h-24 flex-shrink-0 bg-black/40 rounded-lg overflow-hidden relative"
                                :title="$t('Open :map on defrag.racing', { map: row.map })"
                                @click="openMap(row.map)"
                            >
                                <img
                                    :src="row.thumbnail || FALLBACK_THUMBNAIL"
                                    :alt="row.map"
                                    class="w-full h-full object-cover object-center"
                                    loading="lazy"
                                    @error="onThumbnailError"
                                />
                                <span class="absolute top-1 left-1 px-1.5 py-0.5 rounded bg-black/70 text-[10px] uppercase font-semibold text-neutral-200">
                                    {{ row.physics }}
                                </span>
                            </button>
                            <div class="min-w-0 py-2 flex-1">
                                <button
                                    class="text-base font-semibold text-brand-300 hover:underline truncate max-w-full text-left block"
                                    :title="$t('Open :map on defrag.racing', { map: row.map })"
                                    @click="openMap(row.map)"
                                >{{ row.map }}</button>
                                <div v-if="row.author" class="text-xs text-neutral-400 truncate">
                                    {{ $t('by :author', { author: row.author }) }}
                                </div>
                                <div class="text-[11px] text-neutral-500 mt-0.5">
                                    {{ row.entrants === 1 ? $t('1 player entered') : $t(':count players entered', { count: row.entrants }) }}
                                </div>
                            </div>
                            <button
                                class="my-2 mr-2 px-2.5 py-1 self-center rounded text-xs font-semibold bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 flex-shrink-0"
                                :disabled="!config.config.engine_path"
                                :title="config.config.engine_path ? $t('Play :map in :physics', { map: row.map, physics: row.physics.toUpperCase() }) : $t('Pick an engine in Settings first')"
                                @click="playMap(row.map, row.physics)"
                            >{{ $t('Play') }}</button>
                        </div>
                    </div>

                    <!-- The user's own entries. Their own times, which are
                         theirs to see; the invalid_reason arrives as a finished
                         sentence so the launcher needs to know nothing about
                         the rules to explain a refusal. -->
                    <div class="px-3 pb-3">
                        <div class="text-[10px] uppercase text-neutral-500 mb-1">{{ $t('Your entries') }}</div>
                        <table v-if="playing.my_entries.length" class="w-full text-xs">
                            <tbody>
                                <tr
                                    v-for="e in playing.my_entries"
                                    :key="e.id"
                                    class="border-t border-white/[0.03]"
                                >
                                    <td class="py-1 pr-2 text-neutral-300 truncate max-w-[18rem]" :title="e.filename || ''">
                                        {{ e.filename || $t('(demo)') }}
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
                            {{ $t('Nothing entered yet. Record a run on one of the maps above and the launcher will offer to enter it.') }}
                        </p>
                        <p v-if="entered.length" class="text-[11px] text-neutral-500 mt-2">
                            {{ entered.length === 1
                                ? $t('1 demo sent from this machine this session.')
                                : $t(':count demos sent from this machine this session.', { count: entered.length }) }}
                            {{ $t('Entries stay private until the round ends, then they are published like any other demo.') }}
                        </p>
                        <p class="text-[11px] text-neutral-500 mt-2">{{ guardModeNote }}</p>
                    </div>
                </section>

                <section v-else class="bg-neutral-900/40 border border-white/10 rounded-lg p-6 text-center">
                    <div class="text-sm text-neutral-300">{{ $t('No round is being played right now.') }}</div>
                    <p class="text-xs text-neutral-500 mt-1">
                        {{ $t('A new weekly starts every Sunday at 20:00 Prague time.') }}
                    </p>
                </section>

                <!-- What the site did with demos it decided not to enter.
                     Outside the round panel on purpose: a demo can be on hold
                     for a map that is still being voted on, which is a week
                     with no round being played at all. -->
                <section v-if="notices.length" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 text-sm font-semibold text-neutral-200">
                        {{ $t('Demos of yours on hold') }}
                    </header>
                    <ul class="p-3 space-y-2.5">
                        <li v-for="n in notices" :key="n.id" class="text-xs">
                            <div class="truncate text-neutral-300" :title="n.filename || ''">
                                {{ n.filename || $t('(demo)') }}
                            </div>
                            <div :class="n.kind === 'unreadable' ? 'text-red-300' : 'text-neutral-500'">
                                {{ n.note }}
                            </div>
                            <div v-if="n.appears_at" class="text-[11px] text-neutral-600">
                                {{ $t('Appears :when', { when: appearsAt(n.appears_at) }) }}
                            </div>
                            <!-- The one case the site cannot explain by itself:
                                 nobody here knows what is wrong with the file,
                                 so it hands over the person who can look. -->
                            <button
                                v-if="n.kind === 'unreadable'"
                                class="mt-1 text-[11px] text-brand-400 hover:underline"
                                @click="openComps"
                            >{{ $t('Tell the admin on defrag.racing →') }}</button>
                        </li>
                    </ul>
                </section>

                <!-- Where the money comes from. A competition that never says
                     so quietly reads as something the site owes everybody.
                     Who donated stays on the website - this is the total and a
                     way to add to it. -->
                <section v-if="pool?.total_eur" class="bg-neutral-900/40 border border-white/10 rounded-lg px-3 py-2.5 flex flex-wrap items-center gap-x-3 gap-y-1">
                    <span class="text-xs text-neutral-400">
                        {{ $t('Prize pool') }}
                        <span class="text-emerald-300 font-semibold">{{ pool.total_eur.toFixed(2) }} €</span>
                        <span v-if="pool.weeks"> {{ pool.weeks === 1 ? $t('over 1 weekly') : $t('over :count weeklies', { count: pool.weeks }) }}</span>
                        <span v-if="pool.through_comp" class="text-neutral-600">· {{ $t('paid up through weekly :number', { number: pool.through_comp }) }}</span>
                    </span>
                    <button
                        class="ml-auto px-2.5 py-1 rounded text-xs font-semibold bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300"
                        @click="openDonations"
                    >{{ $t('Donate →') }}</button>
                </section>

                <!-- Next week. While the ballot is open it is the five names,
                     and voting happens on the site where the preview videos
                     are. Once it has closed the useful thing is not "closed" -
                     it is what won, when it starts and what it pays. -->
                <section v-if="voting" class="bg-neutral-900/40 border border-white/10 rounded-lg">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between gap-3 flex-wrap">
                        <div class="text-sm font-semibold text-neutral-200">
                            {{ voting.is_open
                                ? $t('Voting for weekly #:number', { number: voting.comp_number })
                                : $t('Next: weekly #:number', { number: voting.comp_number }) }}
                            <span v-if="voting.category" class="text-neutral-500 font-normal">
                                · {{ voting.category }}<span v-if="voting.weapon"> ({{ voting.weapon }})</span>
                            </span>
                            <!-- What next week pays. The reason to go and vote
                                 is usually that the week is worth something. -->
                            <span v-if="voting.prize_eur" class="text-emerald-300 font-normal">
                                · {{ $t(':amount € per physics', { amount: voting.prize_eur.toFixed(2) }) }}
                            </span>
                        </div>
                        <div class="text-xs text-neutral-500 flex items-center gap-3">
                            <span v-if="voting.is_open" :title="closesAtLocal">{{ $t('voting closes :when', { when: closesAtLocal }) }}</span>
                            <span v-else-if="startsIn" :title="startsAtLocal" class="text-neutral-400">
                                {{ $t('starts in :time', { time: startsIn }) }}
                            </span>
                            <span v-else-if="startsAtLocal">{{ $t('starts :when', { when: startsAtLocal }) }}</span>
                            <span v-else>{{ $t('voting closed') }}</span>
                        </div>
                    </header>

                    <!-- What won. The map, who made it and what it looks like:
                         a name on its own is not what anybody came here to
                         find out. -->
                    <div v-if="decidedRows.length" class="p-3 grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div
                            v-for="row in decidedRows"
                            :key="row.physics"
                            class="bg-black/25 border border-white/5 rounded-lg overflow-hidden flex"
                        >
                            <button
                                class="m-2.5 w-32 h-24 flex-shrink-0 bg-black/40 rounded-lg overflow-hidden relative"
                                :title="$t('Open :map on defrag.racing', { map: row.map })"
                                @click="openMap(row.map)"
                            >
                                <img
                                    :src="row.thumbnail || FALLBACK_THUMBNAIL"
                                    :alt="row.map"
                                    class="w-full h-full object-cover object-center"
                                    loading="lazy"
                                    @error="onThumbnailError"
                                />
                                <span class="absolute top-1 left-1 px-1.5 py-0.5 rounded bg-black/70 text-[10px] uppercase font-semibold text-neutral-200">
                                    {{ row.physics }}
                                </span>
                            </button>
                            <div class="p-2.5 min-w-0 flex-1">
                                <button
                                    class="text-base font-semibold text-brand-300 hover:underline truncate max-w-full text-left block"
                                    :title="$t('Open :map on defrag.racing', { map: row.map })"
                                    @click="openMap(row.map)"
                                >{{ row.map }}</button>
                                <div v-if="row.author" class="text-xs text-neutral-400 truncate">
                                    {{ $t('by :author', { author: row.author }) }}
                                </div>
                                <div class="text-[11px] mt-1" :class="row.byWildcard ? 'text-amber-300/90' : 'text-emerald-300/90'">
                                    {{ row.byWildcard
                                        ? $t('picked with a wildcard for :physics', { physics: row.physics.toUpperCase() })
                                        : $t('won the vote for :physics', { physics: row.physics.toUpperCase() }) }}
                                    <span v-if="!row.byWildcard && row.votes !== null" class="text-neutral-500">
                                        · {{ row.votes === 1 ? $t('1 vote') : $t(':count votes', { count: row.votes }) }}
                                    </span>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- The ballot itself, while it is open. Pictures too: this
                         is the list somebody is choosing from. -->
                    <div v-else class="p-3">
                        <div v-if="candidateCards.length" class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
                            <button
                                v-for="c in candidateCards"
                                :key="c.map"
                                class="bg-black/25 border border-white/5 rounded-lg overflow-hidden text-left hover:border-white/20 transition-colors"
                                :title="$t('Open :map on defrag.racing', { map: c.map })"
                                @click="openMap(c.map)"
                            >
                                <!-- One fixed 16:9 box, centre-cropped, and the
                                     site's own stand-in when a map has no
                                     levelshot. Levelshots come in whatever
                                     shape their author saved them in - 4:3
                                     mostly, some square - so they are zoomed to
                                     fill rather than squashed to fit, which is
                                     what made these read as the wrong picture. -->
                                <div class="aspect-video bg-black/40">
                                    <img
                                        :src="c.thumbnail || FALLBACK_THUMBNAIL"
                                        :alt="c.map"
                                        class="w-full h-full object-cover object-center"
                                        loading="lazy"
                                        @error="onThumbnailError"
                                    />
                                </div>
                                <div class="px-2 py-1.5 min-w-0">
                                    <div class="text-xs text-brand-300 truncate">{{ c.map }}</div>
                                    <div v-if="c.author" class="text-[10px] text-neutral-500 truncate">
                                        {{ $t('by :author', { author: c.author }) }}
                                    </div>
                                    <!-- What it is polling, per physics. The
                                         same numbers the site's own ballot
                                         shows, and the final count once the
                                         vote is over. -->
                                    <div v-if="c.votes.length" class="mt-1 flex flex-wrap gap-1">
                                        <span
                                            v-for="v in c.votes"
                                            :key="v.physics"
                                            class="px-1.5 py-0.5 rounded bg-black/40 text-[10px] text-neutral-300"
                                        >
                                            <span class="uppercase text-neutral-500">{{ v.physics }}</span>
                                            <span class="font-semibold ml-1">{{ v.count }}</span>
                                        </span>
                                    </div>
                                </div>
                            </button>
                        </div>
                        <span v-else class="text-xs text-neutral-500">
                            {{ $t('The ballot is not drawn yet.') }}
                        </span>
                    </div>

                    <div class="px-3 pb-3 flex flex-wrap items-center gap-x-2 gap-y-1">
                        <button
                            class="px-3 py-1.5 rounded text-xs font-semibold bg-brand-500/20 hover:bg-brand-500/30 text-brand-300"
                            @click="openComps"
                        >{{ voting.is_open ? $t('Vote on defrag.racing →') : $t('Open comps on defrag.racing →') }}</button>
                        <span v-if="voting.is_open" class="text-[11px] text-neutral-500">
                            {{ $t('Each map has a preview video the launcher cannot play, so voting happens on the site.') }}
                        </span>
                        <span v-else-if="startsAtLocal" class="text-[11px] text-neutral-500">
                            {{ $t('Starts :when.', { when: startsAtLocal }) }}
                            <span v-if="voting.next_category">
                                {{ $t('The week after that is :category.', { category: voting.next_category }) }}
                            </span>
                        </span>
                    </div>
                </section>
            </div>
        </template>
    </div>
</template>
