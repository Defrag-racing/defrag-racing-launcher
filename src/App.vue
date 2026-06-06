<script setup lang="ts">
    import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue';
    import { useRouter, useRoute } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { tauri, type DefragServer } from './lib/tauri';
    import { q3ToHtml } from './lib/q3color';
    import { useConfigStore } from './stores/config';
    import { useNotificationsStore } from './stores/notifications';
    import { useUpdaterStore } from './stores/updater';
    import UpdateBanner from './components/UpdateBanner.vue';

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
        const { openUrl } = await import('@tauri-apps/plugin-opener');
        await openUrl(`https://defrag.racing/profile/${mddId}`).catch(() => {});
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
            launchError.value = e?.toString?.() ?? 'Failed to launch';
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
            launchError.value = e?.toString?.() ?? 'Failed to launch';
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
        const { openUrl } = await import('@tauri-apps/plugin-opener');
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`).catch(() => {});
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

    onMounted(async () => {
        await config.refresh();

        // Bell badge poll. First call goes through immediately so the
        // badge isn't blank on first render; subsequent ticks every
        // 180s hit the lightweight unread-count endpoint (~30B). The
        // poll skips ticks when the window is hidden (launcher in tray)
        // - we still refresh once on `visibilitychange` -> visible so
        // returning users see fresh counts without waiting up to 3min.
        await notifStore.refresh();
        notifPollTimer = window.setInterval(() => {
            if (!document.hidden) notifStore.refresh();
        }, 180_000);
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
                >Demos</RouterLink>
                <RouterLink
                    :to="{ name: 'servers' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'servers'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Servers</RouterLink>
                <RouterLink
                    :to="{ name: 'records' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'records'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Records</RouterLink>
                <RouterLink
                    :to="{ name: 'maps' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'maps'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Maps</RouterLink>
                <RouterLink
                    :to="{ name: 'history' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'history'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >History</RouterLink>
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
                            ? 'Pick an engine in Settings first'
                            : `Quick launch ${config.config.engine_path}`"
                        @click="launchGame"
                    >
                        <span>▶</span>
                        <span>{{ launching ? 'Launching…' : 'Quick launch' }}</span>
                    </button>
                    <button
                        v-if="hasLaunchMenu"
                        class="px-1.5 py-1.5 rounded-r border-l border-emerald-300/20 text-sm bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                        :disabled="!config.config.engine_path || launching"
                        title="Launch a profile"
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
                            <span>▶</span><span>Quick launch</span>
                        </button>
                        <div class="my-1 border-t border-white/[0.06]"></div>
                        <button
                            v-for="p in launchProfiles"
                            :key="p.id"
                            class="w-full text-left px-3 py-1.5 text-sm text-neutral-200 hover:bg-white/5 truncate"
                            :title="p.args"
                            @click="launchProfile(p.args)"
                        >{{ p.name || '(unnamed profile)' }}</button>
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
                    title="Notifications"
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
                        ? `Open profile of ${config.me?.plain_name || config.me?.name} on defrag.racing`
                        : 'Profile link unavailable - paste a token in Settings'"
                    @click="openProfile"
                >Profile</button>
                <RouterLink
                    :to="{ name: 'settings' }"
                    class="px-3 py-1.5 rounded text-sm transition-colors"
                    :class="route.name === 'settings'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Settings</RouterLink>
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
            Loading…
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
                    <div class="font-semibold text-neutral-100">Join server?</div>
                </div>

                <div class="p-5 space-y-3">
                    <!-- Rich server card. Mirrors the Servers tab row. -->
                    <div v-if="pendingServer" class="flex items-start gap-4">
                        <button
                            class="w-36 h-24 rounded bg-black/40 border border-white/10 overflow-hidden flex-shrink-0 hover:border-brand-500/40"
                            :title="`Open ${pendingServer.map} on defrag.racing`"
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
                                no map
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
                                <span>player{{ playerCountOf(pendingServer) === 1 ? '' : 's' }}</span>
                            </div>

                            <!-- Your PB on this map, if any. -->
                            <div v-if="pendingServer.mytime_time" class="text-xs text-emerald-300/85">
                                Your PB: <strong class="font-mono">{{ formatTimeMs(pendingServer.mytime_time) }}</strong>
                                <span v-if="pendingServer.myrank_position && pendingServer.myrank_total" class="text-emerald-300/60 ml-1">
                                    (rank {{ pendingServer.myrank_position }} / {{ pendingServer.myrank_total }})
                                </span>
                            </div>

                            <!-- Map record holder. -->
                            <div
                                v-if="pendingServer.besttime_time && pendingServer.besttime_name"
                                class="text-xs text-yellow-300/75 flex items-center gap-1.5 flex-wrap"
                            >
                                <span class="text-yellow-500">★</span>
                                <span class="font-mono">{{ formatTimeMs(pendingServer.besttime_time) }}</span>
                                <span class="text-neutral-500">by</span>
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
                        <span class="text-neutral-500 uppercase text-[10px] tracking-wider w-full">Players online</span>
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
                        <div class="text-neutral-300 mb-1">Connect to</div>
                        <div class="font-mono text-brand-300 text-base">{{ pendingDeepLink.address }}</div>
                        <div class="text-xs text-neutral-500 mt-1">
                            Server isn't in the live list - private, off-list, or your token can't reach the server browser.
                        </div>
                    </div>

                    <div class="text-xs pt-1">
                        <button
                            class="hover:underline text-neutral-400 hover:text-neutral-200"
                            @click="openAutoConnectSetting"
                        >Skip this confirmation next time →</button>
                    </div>

                    <p v-if="connectError" class="text-xs text-red-300">{{ connectError }}</p>
                </div>

                <div class="px-5 py-3 border-t border-white/10 flex items-center justify-end gap-2 bg-black/30">
                    <button
                        class="px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-sm"
                        @click="cancelConnect"
                    >Dismiss</button>
                    <button
                        class="px-4 py-1.5 rounded bg-brand-500/30 hover:bg-brand-500/40 text-brand-200 font-semibold text-sm disabled:opacity-50"
                        :disabled="connecting"
                        @click="confirmConnect"
                    >{{ connecting ? 'Launching…' : 'Connect' }}</button>
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
                Couldn't open <code class="font-mono">{{ deepLinkError.url }}</code> - {{ deepLinkError.error }}
            </span>
            <button class="text-neutral-300 hover:text-neutral-100" @click="dismissDeepLinkError">×</button>
        </div>
    </div>
</template>
