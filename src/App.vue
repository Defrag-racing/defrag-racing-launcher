<script setup lang="ts">
    import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue';
    import { useRouter, useRoute } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { getCurrentWebview } from '@tauri-apps/api/webview';
    import { tauri, type DefragServer } from './lib/tauri';
    import { q3ToHtml } from './lib/q3color';
    import { useConfigStore } from './stores/config';
    import { useNotificationsStore } from './stores/notifications';
    import { useUpdaterStore } from './stores/updater';
    import UpdateBanner from './components/UpdateBanner.vue';
    import { openExternal } from './lib/open';
    import { loadSeen, notify, saveSeen, type NotifyCategory } from './lib/notify';
    import { resolveLocale, setLocale, t } from './lib/i18n';
    import { locale as osLocale } from '@tauri-apps/plugin-os';

    const router = useRouter();
    const route = useRoute();
    const config = useConfigStore();
    const notifStore = useNotificationsStore();
    const updaterStore = useUpdaterStore();

    // Top nav is hidden during the bootstrap flows (onboarding,
    // version-mismatch screen) - those are full-screen forms that
    // shouldn't be navigable away from. Visible everywhere else so
    // the Play button + tabs are always one click from any view.
    const showNav = computed(() => {
        const r = route.name;
        return r === 'dashboard'
            || r === 'comps'
            || r === 'servers'
            || r === 'records'
            || r === 'maps'
            || r === 'notifications'
            || r === 'history'
            || r === 'settings';
    });

    // Bell badge counts live in a pinia store so the Notifications view
    // can mutate them optimistically on toggle (no flash of stale "5
    // unread" while the API round-trip is in flight). App-level poll
    // every 90s keeps them honest against the server's view in case the
    // user marks something read on the web in another tab.
    let notifPollTimer: number | undefined;

    /// Open the token owner's profile page on defrag.racing. Disabled
    /// when we don't have an mdd_id - either the user hasn't linked
    /// their account to an mdd profile yet, or the launcher hasn't
    /// successfully fetched /api/launcher/me (no token, network
    /// error). Title attr explains why so a disabled button isn't a
    /// mystery.
    const openProfile = async () => {
        const mddId = config.me?.mdd_id;
        if (!mddId) return;
        await openExternal(`https://defrag.racing/profile/${mddId}`).catch(() => {});
    };

    const launching = ref(false);
    const launchError = ref<string | null>(null);
    const showLaunchMenu = ref(false);

    // Developer-mode launch profiles surface as extra entries in a small
    // dropdown next to Quick launch. Only when developer mode is on AND at
    // least one profile exists - otherwise the plain button stands alone.
    const launchProfiles = computed(() =>
        config.config.developer_mode ? (config.config.launch_profiles ?? []) : [],
    );
    const hasLaunchMenu = computed(() => launchProfiles.value.length > 0);

    const launchGame = async () => {
        launchError.value = null;
        launching.value = true;
        showLaunchMenu.value = false;
        try {
            // Apply custom args to the standard Quick launch when developer
            // mode is on; otherwise the plain no-args launch.
            const extra = config.config.developer_mode
                ? (config.config.custom_launch_args ?? '').trim()
                : '';
            if (extra) await tauri.launchEngineArgs(extra);
            else await tauri.launchEngine();
        } catch (e: any) {
            launchError.value = e?.toString?.() ?? t('Failed to launch');
        } finally {
            launching.value = false;
        }
    };

    const launchProfile = async (args: string) => {
        launchError.value = null;
        launching.value = true;
        showLaunchMenu.value = false;
        try {
            await tauri.launchEngineArgs((args ?? '').trim());
        } catch (e: any) {
            launchError.value = e?.toString?.() ?? t('Failed to launch');
        } finally {
            launching.value = false;
        }
    };

    const dismissLaunchError = () => { launchError.value = null; };

    // defrag:// pending-connection modal. Lives at the App level so it
    // floats above whichever tab the user happens to be on - the
    // previous version rendered it inside Dashboard, which meant a
    // forum click while you were on Servers/Records/etc silently
    // dropped a banner in a tab you couldn't see.
    type PendingDeepLink = { address: string; url: string };
    const pendingDeepLink = ref<PendingDeepLink | null>(null);
    const pendingServer = ref<DefragServer | null>(null);
    const connecting = ref(false);
    const connectError = ref<string | null>(null);

    type DeepLinkError = { url: string; error: string };
    const deepLinkError = ref<DeepLinkError | null>(null);
    let deepLinkErrorTimer: number | undefined;

    const physicsOfServer = (s: DefragServer): 'vq3' | 'cpm' =>
        s.defrag?.toLowerCase().includes('cpm') ? 'cpm' : 'vq3';
    const playerCountOf = (s: DefragServer): number => s.online_players?.length ?? 0;
    const thumbnailUrlOf = (s: DefragServer): string | null => {
        const t = s.mapdata?.thumbnail;
        if (!t) return null;
        if (t.startsWith('http://') || t.startsWith('https://')) return t;
        return `https://defrag.racing/storage/${t}`;
    };
    const flagUrlOf = (country: string | null | undefined): string | null => {
        if (!country) return null;
        if (country === '_404' || country === 'XX') return null;
        return `https://defrag.racing/images/flags/${country.toLowerCase()}.png`;
    };

    /** Mirror of Servers.vue::gametypeTag - the short uppercase pill
     *  shown next to the server name. Same fallback chain so the modal
     *  reads consistently with the Servers list. */
    const gametypeTagOf = (s: DefragServer): string => {
        const serverName = (s.plain_name || '').toLowerCase();
        const mixed = String(s.defrag_gametype) === '5' || serverName.includes('mixed');
        if (mixed) return 'MIXED';
        const t = (s.type || 'run').toLowerCase();
        if (t === 'ctf') return 'CTF';
        if (t === 'freestyle') return 'FREESTYLE';
        if (t === 'teamrun') return 'TEAMRUN';
        return 'RUN';
    };

    /** Defrag times in ms → MM:SS.mmm / SS.mmm. */
    const formatTimeMs = (ms: number | null | undefined): string => {
        if (ms == null || ms <= 0) return '-';
        const totalSec = Math.floor(ms / 1000);
        const m = Math.floor(totalSec / 60);
        const s = totalSec % 60;
        const mmm = ms % 1000;
        if (m > 0) return `${m}:${s.toString().padStart(2, '0')}.${mmm.toString().padStart(3, '0')}`;
        return `${s}.${mmm.toString().padStart(3, '0')}`;
    };

    const stripQ3Colors = (s: string | null | undefined): string =>
        (s ?? '').replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '');

    const openServerMap = async (mapname: string | undefined) => {
        if (!mapname) return;
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`).catch(() => {});
    };

    const lookupPendingServer = async (address: string) => {
        pendingServer.value = null;
        if (!config.hasToken) return;
        try {
            const resp = await tauri.getServers();
            const [ip, portStr] = address.split(':');
            const port = Number(portStr);
            pendingServer.value = (resp.servers ?? []).find(
                (s) => s.ip === ip && s.port === port,
            ) ?? null;
        } catch {
            pendingServer.value = null;
        }
    };

    const confirmConnect = async () => {
        connectError.value = null;
        connecting.value = true;
        try {
            const enrichment = pendingServer.value
                ? {
                      map: pendingServer.value.map,
                      server_name: pendingServer.value.name || pendingServer.value.plain_name || null,
                      physics: physicsOfServer(pendingServer.value),
                  }
                : undefined;
            await tauri.confirmPendingDeepLink(enrichment);
            pendingDeepLink.value = null;
            pendingServer.value = null;
        } catch (e: any) {
            connectError.value = e?.toString?.() ?? 'Connect failed';
        } finally {
            connecting.value = false;
        }
    };

    const cancelConnect = async () => {
        await tauri.cancelPendingDeepLink();
        pendingDeepLink.value = null;
        pendingServer.value = null;
        connectError.value = null;
    };

    const dismissDeepLinkError = () => {
        deepLinkError.value = null;
        window.clearTimeout(deepLinkErrorTimer);
    };

    const openAutoConnectSetting = async () => {
        await router.push('/settings');
        await nextTick();
        document
            .getElementById('deep-link-auto-connect')
            ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    };

    let unlistenPending: UnlistenFn | null = null;
    let unlistenResult: UnlistenFn | null = null;
    let unlistenQueue: UnlistenFn | null = null;
    let unlistenDrop: UnlistenFn | null = null;

    // ---- demos dragged onto the window ------------------------------
    // The drop is caught here rather than in the Demos view because the drag
    // lands wherever the user happens to be - the whole window is the target,
    // and the overlay has to be visible on the Servers tab too. The Demos view
    // does the actual work; it stays mounted behind the other tabs (KeepAlive),
    // so a plain window event reaches it from here.
    //
    // Anything ending in `.dm_<number>` counts. The engine writes `.dm_68`
    // today, but older protocols wrote 66 and 67 and those demos still exist
    // on people's disks.
    const DEMO_FILE = /\.dm_\d+$/i;
    const demoFilesIn = (paths: string[]) => paths.filter((p) => DEMO_FILE.test(p));

    /** Set while a drag is over the window: how many demos we would take, or
     *  null for a drag holding none. Drives the overlay and nothing else. */
    const dropHint = ref<{ demos: number } | null>(null);
    const dropRefusal = ref(false);

    /** Up to four demos play side by side, and only with a linked account -
     *  so say what will really happen rather than what was dropped. */
    const MAX_DROP_COMPARE = 4;
    const dropTaken = computed(() => {
        const n = dropHint.value?.demos ?? 0;
        return config.hasToken ? Math.min(n, MAX_DROP_COMPARE) : Math.min(n, 1);
    });
    const dropHeadline = computed(() =>
        dropTaken.value > 1
            ? t('Compare these :count demos', { count: dropTaken.value })
            : t('Play this demo'),
    );
    const dropDetail = computed(() => {
        const dropped = dropHint.value?.demos ?? 0;
        if (dropTaken.value > 1) {
            return dropped > dropTaken.value
                ? t('Drop anywhere. Four is the most that fit, so the first :count open.', { count: dropTaken.value })
                : t('Drop anywhere to open them side by side.');
        }
        if (dropped > 1 && ! config.hasToken) {
            return t('Drop anywhere. Comparing runs needs a linked account, so the first one plays.');
        }
        return t('Drop anywhere to open it in the player.');
    });

    // Demos the comps guard is holding. Badged on the Comps tab because a
    // held demo is doing nothing until someone answers it, and the user has
    // no other reason to look - they recorded a run and expect the launcher
    // to have dealt with it.
    const heldForComps = ref(0);
    const countHeld = (items: { status: string }[]) =>
        items.filter((i) => i.status === 'held_for_comps').length;

    // ---- desktop notifications --------------------------------------
    // Everything below is about somebody who is not looking at this window.
    // The launcher lives minimised behind a fullscreen game, so a round that
    // opened, a demo waiting for an answer and a record taken off you all have
    // to travel out of the app or they arrive after they mattered.

    /** One place where the master switch and the per-category switch are both
     *  checked, so no caller can forget one of them. */
    const announce = (category: NotifyCategory, title: string, body: string) => {
        if (! config.config.notify_enabled) return;
        if (category === 'comps' && ! config.config.notify_comps) return;
        if (category === 'records' && ! config.config.notify_records) return;
        if (category === 'system' && ! config.config.notify_system) return;
        void notify(title, body);
    };

    const notifyWanted = () =>
        config.config.notify_enabled
        && (config.config.notify_comps || config.config.notify_records || config.config.notify_system);

    /** Strip Q3 colour codes - a notification is plain text and `^1neyo` in a
     *  system toast reads as a typo. */
    const plain = (s: string | null | undefined) =>
        (s ?? '').replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '').trim();

    /** Times arrive as milliseconds. */
    const asTime = (ms: number | null | undefined) => {
        if (ms == null || ms <= 0) return '';
        const total = Math.floor(ms / 1000);
        const m = Math.floor(total / 60);
        const s = total % 60;
        const mmm = (ms % 1000).toString().padStart(3, '0');
        return m > 0 ? `${m}:${s.toString().padStart(2, '0')}.${mmm}` : `${s}.${mmm}`;
    };

    /** A demo held by the comps guard is the one message the user cannot act on
     *  from inside the game, and the one they are least likely to be watching
     *  for: they finished a run and assume the launcher dealt with it.
     *
     *  Only the arrival is announced. A demo the user themselves entered or
     *  released does not need a toast - they were looking at the app when they
     *  did it. */
    type QueueRow = { status: string; path: string; filename: string };
    let heldPaths: Set<string> | null = null;

    const onQueueSnapshot = (items: QueueRow[]) => {
        heldForComps.value = countHeld(items);
        const held = items.filter((i) => i.status === 'held_for_comps');
        const paths = new Set(held.map((i) => i.path));

        // The first snapshot is the state at startup, not news. Announcing it
        // would replay every demo held before the launcher was even opened.
        if (heldPaths === null) {
            heldPaths = paths;
            return;
        }
        for (const row of held) {
            if (! heldPaths.has(row.path)) {
                announce(
                    'comps',
                    'A demo is waiting for you',
                    `${row.filename} looks like a run of this week's map. It is being held until you say whether it goes into the round.`,
                );
            }
        }
        heldPaths = paths;
    };

    /** The site's own feed: records taken, and everything else it sends.
     *
     *  Only ever announces what is newer than the highest id already seen, and
     *  a source whose id is still zero is seeded silently. Without that, a
     *  fresh install would open with every unread notification the account has
     *  ever collected arriving at once. */
    const checkSiteNotifications = async () => {
        if (! config.hasToken || ! notifyWanted()) return;
        let feed;
        try {
            feed = await tauri.getNotifications();
        } catch {
            return; // offline, or the token went away. Next tick.
        }
        const seen = await loadSeen();
        const newestRecord = feed.records[0]?.id ?? 0;
        const newestSystem = feed.system[0]?.id ?? 0;

        if (seen.record > 0) {
            // Oldest first, so a burst reads in the order it happened. Capped:
            // three toasts is a report, ten is an assault.
            const fresh = feed.records.filter((r) => r.id > seen.record).reverse();
            for (const r of fresh.slice(0, 3)) {
                const who = plain(r.name) || 'Someone';
                const map = r.mapname ?? 'a map';
                const time = asTime(r.time);
                announce(
                    'records',
                    r.worldrecord ? 'World record taken' : 'Your time was beaten',
                    time ? `${who} on ${map} - ${time}` : `${who} on ${map}`,
                );
            }
            if (fresh.length > 3) {
                announce('records', 'More records', `${fresh.length - 3} more of your times were beaten.`);
            }
        }

        if (seen.system > 0) {
            const fresh = feed.system.filter((s) => s.id > seen.system).reverse();
            for (const s of fresh.slice(0, 3)) {
                const line = [s.before, s.headline, s.after].map(plain).filter(Boolean).join(' ');
                announce('system', 'defrag.racing', line || 'You have a new notification.');
            }
            if (fresh.length > 3) {
                announce('system', 'defrag.racing', `${fresh.length - 3} more notifications.`);
            }
        }

        await saveSeen({ record: newestRecord, system: newestSystem });
    };

    /** Comps: a round opening, an entry settling, and results landing.
     *
     *  Served from the launcher's cached copy, which the backend refreshes on
     *  its own every few minutes, so this costs nothing on most ticks. */
    const checkComps = async () => {
        if (! config.hasToken || ! config.config.notify_enabled || ! config.config.notify_comps) return;
        let payload;
        try {
            payload = await tauri.getComps();
        } catch {
            return;
        }
        const seen = await loadSeen();
        const playing = payload.playing;
        const roundId = playing?.round_id ?? 0;

        // The round you had a run in is gone, so it is over and the times are
        // public. Announced before the new round, because that is the order it
        // happened in.
        if (seen.enteredRound > 0 && roundId !== seen.enteredRound) {
            announce(
                'comps',
                'The round you entered is over',
                'Results are up on defrag.racing.',
            );
            await saveSeen({ enteredRound: 0 });
        }

        if (roundId > 0 && roundId !== seen.round) {
            if (seen.round > 0) {
                const maps = Object.entries(playing?.maps ?? {})
                    .filter(([, m]) => !!m)
                    .map(([physics, m]) => `${physics.toUpperCase()}: ${m}`)
                    .join(' · ');
                announce(
                    'comps',
                    `Week ${playing?.comp_number ?? ''} is open`.replace(/\s+$/, ''),
                    maps || 'A new round has started.',
                );
            }
            await saveSeen({ round: roundId });
        }

        const entries = playing?.my_entries ?? [];
        if (entries.length && roundId > 0) {
            await saveSeen({ enteredRound: roundId });
        }

        // A verdict on a run you put in. Pending says nothing - it is the
        // state a run sits in while nothing has happened to it.
        const settled = (await loadSeen()).settled;
        const fresh = entries.filter((e) => e.status !== 'pending' && ! settled.includes(e.id));
        if (fresh.length && seen.round > 0) {
            for (const e of fresh) {
                const where = e.physics ? ` (${e.physics.toUpperCase()})` : '';
                if (e.status === 'valid') {
                    announce('comps', 'Your run is in', `${e.time ?? 'Your time'}${where} counts for this round.`);
                } else {
                    announce(
                        'comps',
                        'Your run did not count',
                        e.invalid_reason || `The run${where} was rejected.`,
                    );
                }
            }
        }
        if (fresh.length) {
            await saveSeen({ settled: [...settled, ...fresh.map((e) => e.id)] });
        }
    };

    /** Unread total at the last tick, so the feed is only re-read when
     *  something about it changed. `null` means "not asked yet". */
    let prevUnread: number | null = null;

    const notifTick = async () => {
        const wanted = notifyWanted() && config.hasToken;
        // The badge poll used to skip a hidden window, on the grounds that
        // nobody was looking at the badge. Notifications exist for exactly that
        // case, so with them on the tick runs regardless.
        if (! document.hidden || wanted) await notifStore.refresh();
        if (! wanted) {
            prevUnread = null;
            return;
        }
        if (prevUnread === null || notifStore.total !== prevUnread) {
            prevUnread = notifStore.total;
            await checkSiteNotifications();
        }
        await checkComps();
    };

    onMounted(async () => {
        await config.refresh();

        // Language before anything renders text worth reading. A saved choice
        // wins; otherwise the OS decides, because somebody on a Czech Windows
        // did not choose English, they just never opened Settings - and a
        // launcher that opens in a language you cannot read is a launcher whose
        // Settings you cannot find.
        // The OS locale is a plugin call, and a plugin call the capability
        // file does not allow just throws - which is how "Same as my system"
        // quietly meant English for a whole release. Say so in the log rather
        // than swallowing it.
        const systemLocale = await osLocale().catch((e: any) => {
            void tauri.logToFile(`i18n: could not read the system locale: ${e}`);
            return null;
        });
        setLocale(resolveLocale(config.config.language, systemLocale));

        // Bell badge poll. First call goes through immediately so the
        // badge isn't blank on first render; subsequent ticks every
        // 180s hit the lightweight unread-count endpoint (~30B). The
        // poll skips ticks when the window is hidden (launcher in tray)
        // - we still refresh once on `visibilitychange` -> visible so
        // returning users see fresh counts without waiting up to 3min.
        await notifTick();
        notifPollTimer = window.setInterval(() => { void notifTick(); }, 180_000);
        document.addEventListener('visibilitychange', () => {
            if (!document.hidden) notifStore.refresh();
        });

        // Updater: boot check + recurring interval. Lives at App
        // level so a tab switch doesn't reset the cadence and both
        // Settings (countdown UI) + Dashboard (banner) read the same
        // shared state.
        if (config.config.auto_update_enabled) {
            void updaterStore.start();
        }

        // Pending defrag:// listeners. Live event for "user clicked a
        // forum link while launcher is open"; cold-start path for "the
        // launcher just spawned because of that click".
        unlistenPending = await listen<PendingDeepLink>('deep-link://pending', (ev) => {
            pendingDeepLink.value = ev.payload;
            connectError.value = null;
            void lookupPendingServer(ev.payload.address);
        });
        unlistenResult = await listen<{ ok: false; url: string; error: string }>(
            'deep-link://result',
            (ev) => {
                if (!ev.payload.ok) {
                    deepLinkError.value = { url: ev.payload.url, error: ev.payload.error };
                    window.clearTimeout(deepLinkErrorTimer);
                    deepLinkErrorTimer = window.setTimeout(() => { deepLinkError.value = null; }, 6000);
                }
            },
        );
        unlistenQueue = await listen<{ items: QueueRow[] }>(
            'upload_state_changed',
            (ev) => { onQueueSnapshot(ev.payload.items); },
        );
        try {
            const snapshot = await tauri.getUploadState();
            onQueueSnapshot(snapshot.items);
        } catch { /* no queue yet */ }

        unlistenDrop = await getCurrentWebview().onDragDropEvent((ev) => {
            // Not during onboarding or the version-mismatch screen: those are
            // full-screen forms, and the Demos view they would hand off to may
            // not exist yet.
            if (! showNav.value) return;

            const p = ev.payload;
            if (p.type === 'enter') {
                const demos = demoFilesIn(p.paths);
                dropHint.value = demos.length ? { demos: demos.length } : null;
                dropRefusal.value = demos.length === 0;
            } else if (p.type === 'leave') {
                dropHint.value = null;
                dropRefusal.value = false;
            } else if (p.type === 'drop') {
                dropHint.value = null;
                dropRefusal.value = false;
                const demos = demoFilesIn(p.paths);
                if (demos.length) {
                    window.dispatchEvent(new CustomEvent('demos-dropped', { detail: demos }));
                }
            }
        });

        try {
            const url = await tauri.getPendingDeepLink();
            if (url && !pendingDeepLink.value) {
                const m = url.match(/^defrag:\/\/([^/]+)/);
                pendingDeepLink.value = { url, address: m ? m[1] : url };
                void lookupPendingServer(pendingDeepLink.value.address);
            }
        } catch { /* no-op */ }

        // Upgrade-aware boot flow:
        //  1. Fresh install (no onboarding) → onboarding wizard
        //  2. Config left behind by an older launcher → mismatch screen
        //     (user picks keep-or-wipe before the dashboard)
        //  3. Normal same-version boot → dashboard (via the default route)
        if (! config.config.onboarding_completed) {
            router.replace({ name: 'onboarding' });
            return;
        }

        const previous = await tauri.previousVersion();
        if (previous) {
            const current = await tauri.appVersion();
            router.replace({
                name: 'version-mismatch',
                query: { previous, current },
            });
        }
    });

    onUnmounted(() => {
        if (notifPollTimer !== undefined) window.clearInterval(notifPollTimer);
        if (unlistenPending) unlistenPending();
        if (unlistenResult) unlistenResult();
        if (unlistenQueue) unlistenQueue();
        if (unlistenDrop) unlistenDrop();
        window.clearTimeout(deepLinkErrorTimer);
        updaterStore.stop();
    });
</script>

<template>
    <div class="h-full flex flex-col">
        <!-- Top nav: tabs on the left, Play CTA on the right. Sticky to
             the top of the window so it never scrolls out of view, and
             the Play button stays one click away from any tab. -->
        <nav
            v-if="showNav"
            class="flex items-center justify-between border-b border-white/10 bg-neutral-950 px-3 h-11 flex-shrink-0"
        >
            <div class="flex items-center gap-1">
                <RouterLink
                    :to="{ name: 'dashboard' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'dashboard'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('Demos') }}</RouterLink>
                <!-- Comps sits next to Demos on purpose: a held demo is a
                     demo, and the answer to "where did my run go" has to be
                     one tab away from where the user looked first. The dot
                     appears while something is waiting on an answer. -->
                <RouterLink
                    :to="{ name: 'comps' }"
                    class="relative px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'comps'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >
                    {{ $t('Comps') }}
                    <span
                        v-if="heldForComps > 0"
                        class="absolute -top-0.5 -right-0.5 text-[10px] font-bold px-1 min-w-[16px] h-[16px] flex items-center justify-center rounded-full bg-amber-500 text-black"
                        :title="$t(':count demos are waiting for you to choose', { count: heldForComps })"
                    >{{ heldForComps }}</span>
                </RouterLink>
                <RouterLink
                    :to="{ name: 'servers' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'servers'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('Servers') }}</RouterLink>
                <RouterLink
                    :to="{ name: 'records' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'records'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('Records') }}</RouterLink>
                <RouterLink
                    :to="{ name: 'maps' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'maps'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('Maps') }}</RouterLink>
                <RouterLink
                    :to="{ name: 'history' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'history'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('History') }}</RouterLink>
            </div>

            <div class="flex items-center gap-2">
                <!-- Play CTA. Big, green, labelled - this is the "I
                     want to launch the game right now" button. Disabled
                     with a tooltip when the engine path isn't set so
                     the user knows where to go fix it. In developer mode
                     with launch profiles, a caret opens a menu to launch
                     a specific profile instead. -->
                <div class="relative flex items-center">
                    <button
                        class="px-3 py-1.5 text-sm font-semibold flex items-center gap-1.5 bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                        :class="hasLaunchMenu ? 'rounded-l' : 'rounded'"
                        :disabled="!config.config.engine_path || launching"
                        :title="!config.config.engine_path
                            ? $t('Pick an engine in Settings first')
                            : $t('Quick launch :path', { path: config.config.engine_path })"
                        @click="launchGame"
                    >
                        <span>▶</span>
                        <span>{{ launching ? $t('Launching…') : $t('Quick launch') }}</span>
                    </button>
                    <button
                        v-if="hasLaunchMenu"
                        class="px-1.5 py-1.5 rounded-r border-l border-emerald-300/20 text-sm bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                        :disabled="!config.config.engine_path || launching"
                        :title="$t('Launch a profile')"
                        @click="showLaunchMenu = !showLaunchMenu"
                    >▾</button>

                    <!-- Click-away backdrop + dropdown. -->
                    <div v-if="showLaunchMenu" class="fixed inset-0 z-40" @click="showLaunchMenu = false"></div>
                    <div
                        v-if="showLaunchMenu"
                        class="absolute right-0 top-full mt-1 z-50 w-56 bg-neutral-900 border border-white/10 rounded-lg shadow-xl overflow-hidden py-1"
                    >
                        <button
                            class="w-full text-left px-3 py-1.5 text-sm text-emerald-300 hover:bg-white/5 flex items-center gap-2"
                            @click="launchGame"
                        >
                            <span>▶</span><span>{{ $t('Quick launch') }}</span>
                        </button>
                        <div class="my-1 border-t border-white/[0.06]"></div>
                        <button
                            v-for="p in launchProfiles"
                            :key="p.id"
                            class="w-full text-left px-3 py-1.5 text-sm text-neutral-200 hover:bg-white/5 truncate"
                            :title="p.args"
                            @click="launchProfile(p.args)"
                        >{{ p.name || $t('(unnamed profile)') }}</button>
                    </div>
                </div>

                <!-- Notifications bell. Badge shows unread total
                     across record + system feeds; refreshed every
                     90s by the App-level poll. -->
                <RouterLink
                    :to="{ name: 'notifications' }"
                    class="relative px-3 py-1.5 rounded text-sm transition-colors"
                    :class="route.name === 'notifications'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                    :title="$t('Notifications')"
                >
                    <span>🔔</span>
                    <span
                        v-if="notifStore.total > 0"
                        class="absolute -top-1 -right-1 text-[10px] font-bold px-1 min-w-[16px] h-[16px] flex items-center justify-center rounded-full bg-brand-500 text-white"
                    >{{ notifStore.total > 99 ? '99+' : notifStore.total }}</span>
                </RouterLink>

                <!-- Profile: opens the token owner's defrag.racing
                     profile page in the default browser. Disabled
                     when we don't yet know who the user is (no
                     token, /api/launcher/me failed, or no linked
                     mdd profile). -->
                <button
                    class="px-3 py-1.5 rounded text-sm transition-colors text-neutral-400 hover:text-neutral-200 hover:bg-white/5 disabled:opacity-40 disabled:cursor-not-allowed"
                    :disabled="!config.me?.mdd_id"
                    :title="config.me?.mdd_id
                        ? $t('Open :name on defrag.racing', { name: config.me?.plain_name || config.me?.name || '' })
                        : $t('Profile link unavailable - paste a token in Settings')"
                    @click="openProfile"
                >{{ $t('Profile') }}</button>
                <RouterLink
                    :to="{ name: 'settings' }"
                    class="px-3 py-1.5 rounded text-sm transition-colors"
                    :class="route.name === 'settings'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >{{ $t('Settings') }}</RouterLink>
            </div>
        </nav>

        <p
            v-if="launchError"
            class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300 flex items-center gap-2 flex-shrink-0"
        >
            <span>{{ launchError }}</span>
            <button class="ml-auto text-neutral-400 hover:text-neutral-200" @click="dismissLaunchError">×</button>
        </p>

        <!-- Update banner - app-level so it shows on every tab, not just
             Demos. Hidden on the settings route, which mounts its own copy
             inside the "Check now" card (so the two never stack). Only
             renders when an update is actually available / in progress, so
             it adds no chrome the rest of the time. showNav gates it to the
             same screens as the nav, keeping it off the onboarding /
             version-mismatch full-screen flows. -->
        <UpdateBanner v-if="showNav && route.name !== 'settings'" />

        <!-- keep-alive: cached views survive tab switches so paginated
             lists + filters + scroll positions stay intact. Each view
             owns its own poll (onActivated starts, onDeactivated stops)
             so a backgrounded tab doesn't keep hitting the API. -->
        <RouterView v-if="config.loaded" v-slot="{ Component }">
            <KeepAlive>
                <component :is="Component" />
            </KeepAlive>
        </RouterView>
        <div v-else class="flex-1 flex items-center justify-center text-sm text-neutral-500">
            {{ $t('Loading…') }}
        </div>

        <!-- Drop a demo anywhere on the window. Above every tab because the
             drag lands wherever the user is; pointer-events off so it never
             eats the drop it is describing. -->
        <div
            v-if="dropHint || dropRefusal"
            class="fixed inset-0 z-[120] flex items-center justify-center p-8 bg-black/70 backdrop-blur-sm pointer-events-none"
        >
            <div
                class="w-full max-w-md rounded-xl border-2 border-dashed px-8 py-10 text-center"
                :class="dropRefusal
                    ? 'border-neutral-600 bg-neutral-900/80'
                    : 'border-brand-500 bg-brand-500/10'"
            >
                <template v-if="dropRefusal">
                    <p class="text-base font-semibold text-neutral-200">{{ $t('That is not a demo') }}</p>
                    <p class="mt-2 text-sm text-neutral-400">
                        {{ $t('Drop a Quake 3 demo file here - the ones ending in :extension.', { extension: '.dm_68' }) }}
                    </p>
                </template>
                <template v-else-if="dropHint">
                    <p class="text-base font-semibold text-white">{{ dropHeadline }}</p>
                    <p class="mt-2 text-sm text-neutral-300">{{ dropDetail }}</p>
                </template>
            </div>
        </div>

        <!-- defrag:// pending-connection modal. Floats above the whole
             launcher so the user sees it regardless of which tab is
             active. Layout matches the Servers row so the modal reads
             as "this is the server you'd see in the browser" rather
             than a bare IP prompt: bigger map thumbnail + gametype
             tag + Your PB + map record holder + player list. -->
        <div
            v-if="pendingDeepLink"
            class="fixed inset-0 z-[100] flex items-center justify-center p-4 bg-black/70 backdrop-blur-sm overflow-auto"
            @click.self="cancelConnect"
        >
            <div class="w-full max-w-xl my-auto bg-neutral-900 border border-brand-500/30 rounded-lg shadow-2xl overflow-hidden">
                <div class="px-5 py-3 border-b border-white/10 flex items-center gap-2">
                    <span class="text-brand-400">🎮</span>
                    <div class="font-semibold text-neutral-100">{{ $t('Join server?') }}</div>
                </div>

                <div class="p-5 space-y-3">
                    <!-- Rich server card. Mirrors the Servers tab row. -->
                    <div v-if="pendingServer" class="flex items-start gap-4">
                        <button
                            class="w-36 h-24 rounded bg-black/40 border border-white/10 overflow-hidden flex-shrink-0 hover:border-brand-500/40"
                            :title="$t('Open :map on defrag.racing', { map: pendingServer.map })"
                            @click="openServerMap(pendingServer.map)"
                        >
                            <img
                                v-if="thumbnailUrlOf(pendingServer)"
                                :src="thumbnailUrlOf(pendingServer)!"
                                :alt="pendingServer.map"
                                class="w-full h-full object-cover"
                                loading="lazy"
                            />
                            <div v-else class="w-full h-full flex items-center justify-center text-[10px] text-neutral-600 uppercase">
                                {{ $t('no map') }}
                            </div>
                        </button>

                        <div class="flex-1 min-w-0 space-y-1">
                            <!-- Server name + flag + gametype tag + physics pill. -->
                            <div class="flex items-center gap-2 min-w-0 flex-wrap">
                                <img
                                    v-if="flagUrlOf(pendingServer.location)"
                                    :src="flagUrlOf(pendingServer.location)!"
                                    :alt="pendingServer.location || ''"
                                    class="w-4 h-3 rounded flex-shrink-0"
                                    @error="($event.target as HTMLImageElement).style.display='none'"
                                />
                                <div
                                    class="text-sm text-neutral-100 truncate font-semibold"
                                    :title="stripQ3Colors(pendingServer.name || pendingServer.plain_name)"
                                    v-html="q3ToHtml(pendingServer.name || pendingServer.plain_name)"
                                ></div>
                                <span class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-brand-500/15 text-brand-300 flex-shrink-0">
                                    {{ gametypeTagOf(pendingServer) }}
                                </span>
                                <span class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5 text-neutral-300 flex-shrink-0">
                                    {{ physicsOfServer(pendingServer) }}
                                </span>
                            </div>

                            <!-- Map link + ip:port + players. -->
                            <div class="text-xs text-neutral-400 flex items-center gap-2 flex-wrap">
                                <button class="text-brand-400 hover:underline truncate max-w-[14rem]" @click="openServerMap(pendingServer.map)">
                                    {{ pendingServer.map }}
                                </button>
                                <span class="text-neutral-600">·</span>
                                <span class="font-mono">{{ pendingServer.ip }}:{{ pendingServer.port }}</span>
                                <span class="text-neutral-600">·</span>
                                <span class="text-neutral-300 font-semibold">{{ playerCountOf(pendingServer) }}</span>
                                <span>{{ playerCountOf(pendingServer) === 1 ? $t('player') : $t('players') }}</span>
                            </div>

                            <!-- Your PB on this map, if any. -->
                            <div v-if="pendingServer.mytime_time" class="text-xs text-emerald-300/85">
                                {{ $t('Your PB:') }} <strong class="font-mono">{{ formatTimeMs(pendingServer.mytime_time) }}</strong>
                                <span v-if="pendingServer.myrank_position && pendingServer.myrank_total" class="text-emerald-300/60 ml-1">
                                    {{ $t('(rank :position of :total)', { position: pendingServer.myrank_position, total: pendingServer.myrank_total }) }}
                                </span>
                            </div>

                            <!-- Map record holder. -->
                            <div
                                v-if="pendingServer.besttime_time && pendingServer.besttime_name"
                                class="text-xs text-yellow-300/75 flex items-center gap-1.5 flex-wrap"
                            >
                                <span class="text-yellow-500">★</span>
                                <span class="font-mono">{{ formatTimeMs(pendingServer.besttime_time) }}</span>
                                <span class="text-neutral-500">{{ $t('by') }}</span>
                                <img
                                    v-if="flagUrlOf(pendingServer.besttime_country)"
                                    :src="flagUrlOf(pendingServer.besttime_country)!"
                                    :alt="pendingServer.besttime_country || ''"
                                    class="w-3 h-2 rounded flex-shrink-0"
                                    @error="($event.target as HTMLImageElement).style.display='none'"
                                />
                                <span
                                    class="truncate max-w-[10rem]"
                                    :title="stripQ3Colors(pendingServer.besttime_name)"
                                    v-html="q3ToHtml(pendingServer.besttime_name)"
                                ></span>
                            </div>
                        </div>
                    </div>

                    <!-- Player list under the card. Same shape as
                         Servers row: flag + Q3-colored name, comma-
                         separated wrap. Hidden on empty servers. -->
                    <div
                        v-if="pendingServer && (pendingServer.online_players?.length ?? 0) > 0"
                        class="flex flex-wrap gap-x-3 gap-y-1 text-xs pt-1 border-t border-white/[0.04]"
                    >
                        <span class="text-neutral-500 uppercase text-[10px] tracking-wider w-full">{{ $t('Players online') }}</span>
                        <span
                            v-for="(p, idx) in (pendingServer.online_players ?? [])"
                            :key="`pending-player-${idx}`"
                            class="flex items-center gap-1.5 min-w-0"
                        >
                            <img
                                v-if="flagUrlOf(p.country)"
                                :src="flagUrlOf(p.country)!"
                                :alt="p.country || ''"
                                class="w-3 h-2 rounded flex-shrink-0"
                                @error="($event.target as HTMLImageElement).style.display='none'"
                            />
                            <span
                                class="truncate max-w-[8rem]"
                                :title="stripQ3Colors(p.name)"
                                v-html="q3ToHtml(p.name)"
                            ></span>
                        </span>
                    </div>

                    <!-- Fallback: server not in live list. -->
                    <div v-if="!pendingServer" class="text-sm">
                        <div class="text-neutral-300 mb-1">{{ $t('Connect to') }}</div>
                        <div class="font-mono text-brand-300 text-base">{{ pendingDeepLink.address }}</div>
                        <div class="text-xs text-neutral-500 mt-1">
                            {{ $t("Server isn't in the live list - private, off-list, or your token can't reach the server browser.") }}
                        </div>
                    </div>

                    <div class="text-xs pt-1">
                        <button
                            class="hover:underline text-neutral-400 hover:text-neutral-200"
                            @click="openAutoConnectSetting"
                        >{{ $t('Skip this confirmation next time →') }}</button>
                    </div>

                    <p v-if="connectError" class="text-xs text-red-300">{{ connectError }}</p>
                </div>

                <div class="px-5 py-3 border-t border-white/10 flex items-center justify-end gap-2 bg-black/30">
                    <button
                        class="px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-sm"
                        @click="cancelConnect"
                    >{{ $t('Dismiss') }}</button>
                    <button
                        class="px-4 py-1.5 rounded bg-brand-500/30 hover:bg-brand-500/40 text-brand-200 font-semibold text-sm disabled:opacity-50"
                        :disabled="connecting"
                        @click="confirmConnect"
                    >{{ connecting ? $t('Launching…') : $t('Connect') }}</button>
                </div>
            </div>
        </div>

        <!-- Deep-link error toast: surfaces malformed URLs / engine
             launch failures. Auto-dismisses after 6s; lives at App
             level so it's visible from any tab. -->
        <div
            v-if="deepLinkError"
            class="fixed bottom-4 left-1/2 -translate-x-1/2 z-[90] max-w-md w-[calc(100%-2rem)] px-4 py-3 rounded-lg bg-red-500/15 border border-red-500/30 text-xs text-red-200 flex items-center gap-2 shadow-xl backdrop-blur"
        >
            <span class="flex-1">
                {{ $t("Couldn't open :url", { url: deepLinkError.url }) }} - {{ deepLinkError.error }}
            </span>
            <button class="text-neutral-300 hover:text-neutral-100" @click="dismissDeepLinkError">×</button>
        </div>
    </div>
</template>
