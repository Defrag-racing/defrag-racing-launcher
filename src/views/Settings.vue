<script setup lang="ts">
    import { computed, onActivated, onMounted, onUnmounted, ref } from 'vue';
    import { useRoute, useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openExternal } from '../lib/open';
    import { tauri, type EngineCandidate, type HealthItem, type LaunchProfile } from '../lib/tauri';
    import TokenFeatureList from '../components/TokenFeatureList.vue';
    import TokenFreeFeatures from '../components/TokenFreeFeatures.vue';
    import UpdateBanner from '../components/UpdateBanner.vue';
    import { useConfigStore } from '../stores/config';
    import { useUpdaterStore } from '../stores/updater';
    import { displayPath } from '../lib/path';

    const router = useRouter();
    const route = useRoute();

    // When the Demos / Library "Change in Settings" chip navigates here it
    // passes ?highlight=demos. Pulse + scroll the demos-folder card into
    // view so the user lands looking straight at the field they came to
    // change, instead of having to hunt for it in the settings list.
    // onActivated (not onMounted): this view is cached by <KeepAlive>, so
    // onMounted fires only on the first visit - every later click of the
    // chip would re-enter the cached instance without re-running it. We
    // also clear the query right after so the highlight is a one-shot
    // tied to the chip click, not to merely landing on Settings.
    const highlightDemos = ref(false);
    const highlightToken = ref(false);
    const demosSection = ref<HTMLElement | null>(null);
    const tokenSection = ref<HTMLElement | null>(null);
    let highlightTimer: number | undefined;
    onActivated(async () => {
        // Re-mirror developer-mode fields from the store on every entry so
        // an external change (e.g. a Reset) is reflected.
        syncDevFromConfig();
        const target = route.query.highlight;
        if (target !== 'demos' && target !== 'token') return;
        // Same one-shot pulse + scroll for the token card - the Servers /
        // Records / Maps "Token required" empty states deep-link here with
        // ?highlight=token so the user lands on the field to paste into.
        if (target === 'token') {
            highlightToken.value = true;
            showTokenForm.value = true; // make the input visible if a token already exists
        } else {
            highlightDemos.value = true;
        }
        const section = target === 'token' ? tokenSection : demosSection;
        router.replace({ name: 'settings', query: {} });
        await new Promise((r) => requestAnimationFrame(() => r(null)));
        section.value?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        if (highlightTimer !== undefined) window.clearTimeout(highlightTimer);
        highlightTimer = window.setTimeout(() => {
            highlightDemos.value = false;
            highlightToken.value = false;
        }, 2600);
    });
    const config = useConfigStore();
    const updater = useUpdaterStore();

    // Ticking now-ref so the countdown re-renders each second without
    // forcing the updater store to tick its own clock.
    const nowMs = ref(Date.now());
    let nowTimer: number | undefined;
    onMounted(() => { nowTimer = window.setInterval(() => { nowMs.value = Date.now(); }, 1000); });
    onUnmounted(() => { if (nowTimer !== undefined) window.clearInterval(nowTimer); });

    const countdownLabel = computed(() => {
        if (!updater.lastCheckAt) return '';
        const nextAt = updater.lastCheckAt + updater.intervalMs;
        const s = Math.max(0, Math.ceil((nextAt - nowMs.value) / 1000));
        const m = Math.floor(s / 60);
        const ss = s % 60;
        return `${m}:${ss.toString().padStart(2, '0')}`;
    });

    const manualCheck = () => updater.runCheck('manual');

    const engines = ref<EngineCandidate[]>([]);
    const tokenInput = ref('');
    const tokenSaving = ref(false);
    const tokenError = ref<string | null>(null);
    const showTokenForm = ref(false);

    const appVersion = ref('');
    const reCheckBusy = ref(false);
    const reCheckCooldown = ref(0); // seconds left before the button re-arms
    let reCheckTimer: number | undefined;
    const autostart = ref(false);

    onMounted(async () => {
        syncDevFromConfig();
        engines.value = await tauri.detectEngines();
        appVersion.value = await tauri.appVersion();
        // Read the OS-level autostart state, not just our config -
        // catches the case where the user removed the registration
        // manually (Task Manager → Startup) outside the launcher.
        autostart.value = await tauri.isAutostartEnabled();
    });

    // --- developer mode: custom launch args + named launch profiles -----
    // Local editable copies of the config fields; text inputs and an array
    // editor don't suit the "read store directly, save on @change" pattern
    // the toggles use, so we mirror them here and persist on blur / on
    // structural change. Re-synced from the store on every activation so a
    // Reset or an external config change is reflected.
    const customArgs = ref('');
    const profiles = ref<LaunchProfile[]>([]);

    const syncDevFromConfig = () => {
        customArgs.value = config.config.custom_launch_args ?? '';
        profiles.value = (config.config.launch_profiles ?? []).map((p) => ({ ...p }));
    };

    const toggleDeveloperMode = async (next: boolean) => {
        await config.save({ developer_mode: next });
        syncDevFromConfig();
    };

    const saveCustomArgs = async () => {
        if (customArgs.value === config.config.custom_launch_args) return;
        await config.save({ custom_launch_args: customArgs.value });
    };

    const persistProfiles = async () => {
        // Strip blank rows (no name AND no args) so an abandoned "Add" row
        // doesn't linger as a nameless launch button.
        const cleaned = profiles.value
            .map((p) => ({ id: p.id, name: p.name.trim(), args: p.args.trim() }))
            .filter((p) => p.name !== '' || p.args !== '');
        await config.save({ launch_profiles: cleaned });
        profiles.value = cleaned.map((p) => ({ ...p }));
    };

    const newProfileId = () => {
        // crypto.randomUUID is available in WebView2 / WKWebView / WebKitGTK.
        try { return crypto.randomUUID(); } catch { return `p${profiles.value.length}-${customArgs.value.length}-${profiles.value.reduce((a, p) => a + p.id.length, 0)}`; }
    };

    const addProfile = () => {
        profiles.value.push({ id: newProfileId(), name: '', args: '' });
    };

    const removeProfile = async (id: string) => {
        profiles.value = profiles.value.filter((p) => p.id !== id);
        await persistProfiles();
    };

    const toggleAutostart = async (next: boolean) => {
        try {
            await tauri.setAutostartEnabled(next);
            autostart.value = next;
        } catch (e) {
            // Re-read OS state so the toggle reflects reality even if
            // our write failed (e.g. permission denied on Linux).
            autostart.value = await tauri.isAutostartEnabled();
            alert(`Couldn't change autostart: ${e}`);
        }
    };

    /// Persist the new CPU throttle preference AND push it live into the
    /// running watcher so the change takes effect mid-rescan, not after
    /// the next Stop/Start cycle.
    const setThrottlePreference = async (pct: number) => {
        await config.save({ cpu_throttle_pct: pct });
        try {
            await tauri.setCpuThrottlePctRuntime(pct);
        } catch { /* watcher not running - config save is enough */ }
    };

    const pickEngine = async () => {
        const picked = await openDialog({ multiple: false, directory: false });
        if (typeof picked === 'string') {
            const demos = await tauri.guessDemosPath(picked);
            await config.save({
                engine_path: picked,
                demos_path: demos ?? config.config.demos_path,
            });
        }
    };

    const pickDemos = async () => {
        const picked = await openDialog({ multiple: false, directory: true });
        if (typeof picked === 'string') {
            await config.save({ demos_path: picked });
        }
    };

    const saveToken = async () => {
        if (! tokenInput.value.trim()) return;
        tokenSaving.value = true;
        tokenError.value = null;
        try {
            // Verify with the server before storing, so a wrong-type or
            // invalid token is rejected here with a clear reason instead
            // of being saved and silently failing on the Servers / upload
            // paths later.
            const check = await tauri.validateToken(tokenInput.value.trim());
            if (! check.ok) {
                tokenError.value = check.message;
                return;
            }
            await tauri.saveToken(tokenInput.value.trim());
            tokenInput.value = '';
            showTokenForm.value = false;
            await config.refresh();
        } catch (e: any) {
            tokenError.value = e.toString();
        } finally {
            tokenSaving.value = false;
        }
    };

    const clearToken = async () => {
        if (! confirm('Clear the stored token? Auto-upload will stop until you paste a new one.')) return;
        await tauri.clearToken();
        try { await tauri.stopAutoUpload(); } catch {}
        await config.refresh();
    };

    const forceRecheck = async () => {
        if (reCheckBusy.value || reCheckCooldown.value > 0) return;
        if (! confirm('Re-check every demo against the server? This re-hashes the whole folder and can take a while - watch the progress bar on the Demos tab.')) return;
        reCheckBusy.value = true;
        try {
            await tauri.clearUploadCache();
        } finally {
            reCheckBusy.value = false;
        }
        // Cooldown: each click kicks a full re-hash + server re-verify of the
        // whole folder, so block repeat clicks for a bit (was spammable).
        reCheckCooldown.value = 20;
        if (reCheckTimer !== undefined) window.clearInterval(reCheckTimer);
        reCheckTimer = window.setInterval(() => {
            reCheckCooldown.value -= 1;
            if (reCheckCooldown.value <= 0 && reCheckTimer !== undefined) {
                window.clearInterval(reCheckTimer);
                reCheckTimer = undefined;
            }
        }, 1000);
    };

    onUnmounted(() => {
        if (reCheckTimer !== undefined) window.clearInterval(reCheckTimer);
    });

    // Reset is gated behind a typed confirmation modal rather than a native
    // confirm() - the WebView2 confirm() was unreliable (it could return
    // false without ever showing a dialog, so Reset silently did nothing).
    // The user has to type "yes" / "i understand" to arm the button.
    const showResetConfirm = ref(false);
    const resetConfirmText = ref('');
    const resetting = ref(false);
    const resetConfirmValid = computed(() => {
        const t = resetConfirmText.value.trim().toLowerCase();
        return t === 'yes' || t === 'i understand';
    });
    const cancelReset = () => {
        showResetConfirm.value = false;
        resetConfirmText.value = '';
    };
    const resetLauncher = async () => {
        if (! resetConfirmValid.value || resetting.value) return;
        resetting.value = true;
        try {
            await tauri.resetLauncher();
            await config.refresh();
            showResetConfirm.value = false;
            resetConfirmText.value = '';
            // Back to step 1 of the setup wizard.
            router.replace({ name: 'onboarding' });
        } finally {
            resetting.value = false;
        }
    };

    // -- Check & repair -----------------------------------------------
    const healthItems = ref<HealthItem[]>([]);
    const healthBusy = ref(false);
    const healthRan = ref(false);
    const healthFixing = ref<string | null>(null);
    const runHealthCheck = async () => {
        if (healthBusy.value) return;
        healthBusy.value = true;
        try {
            healthItems.value = await tauri.healthCheck();
            healthRan.value = true;
        } catch (e) {
            healthItems.value = [{ id: 'error', title: 'Check failed', status: 'error', detail: String(e), fix: null }];
            healthRan.value = true;
        } finally {
            healthBusy.value = false;
        }
    };
    const runHealthRepair = async (item: HealthItem) => {
        if (!item.fix || healthFixing.value) return;
        healthFixing.value = item.id;
        try {
            await tauri.healthRepair(item.fix);
            await runHealthCheck(); // re-scan so the row flips to OK
        } catch (e) {
            item.detail = `Repair failed: ${e}`;
        } finally {
            healthFixing.value = null;
        }
    };
    const healthDot = (status: string) =>
        status === 'ok' ? 'bg-emerald-400' : status === 'warn' ? 'bg-amber-400' : 'bg-red-400';
</script>

<template>
    <div class="flex-1 flex flex-col">
        <header class="px-5 py-3 border-b border-white/10 flex items-center gap-3">
            <button class="text-sm text-neutral-400 hover:text-neutral-200" @click="router.back()">← Back</button>
            <h1 class="font-semibold">Settings</h1>
        </header>

        <div class="flex-1 overflow-auto p-5 space-y-4 max-w-2xl w-full">
            <!-- Engine -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Defrag engine</div>
                        <div class="text-xs text-neutral-500 mt-0.5">Used when opening <code class="bg-black/40 px-1 rounded">defrag://</code> links.</div>
                    </div>
                    <button class="btn-ghost" @click="pickEngine">Change</button>
                </div>
                <div class="text-sm text-neutral-300 break-all" :title="config.config.engine_path || ''">
                    {{ displayPath(config.config.engine_path) || '(not set)' }}
                </div>

                <!-- Auto-connect bypass. Off by default so an accidental
                     forum-link click can't yeet you into a random server.
                     Power users who already trust their sources flip this
                     on to skip the confirmation banner. -->
                <div
                    id="deep-link-auto-connect"
                    class="flex items-center justify-between gap-3 pt-3 border-t border-white/[0.05]"
                >
                    <div>
                        <div class="text-sm font-medium">Skip <code class="bg-black/40 px-1 rounded">defrag://</code> confirmation</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Launch the engine immediately without asking. Useful if you
                            join often and trust the links you click. Engine must be set.
                        </div>
                    </div>
                    <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                        <input
                            type="checkbox"
                            class="sr-only peer"
                            :checked="config.config.deep_link_auto_connect"
                            @change="config.save({ deep_link_auto_connect: ($event.target as HTMLInputElement).checked })"
                        />
                        <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                        <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                    </label>
                </div>
            </section>

            <!-- Demos path -->
            <section
                ref="demosSection"
                class="bg-neutral-900 border rounded-lg p-4 space-y-3 transition-all duration-500"
                :class="highlightDemos
                    ? 'border-brand-500/70 ring-2 ring-brand-500/40 shadow-lg shadow-brand-500/10'
                    : 'border-white/10'"
            >
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Demos folder</div>
                        <div class="text-xs text-neutral-500 mt-0.5">The launcher watches this folder for new demos.</div>
                        <div class="text-[11px] text-brand-400/80 mt-1">
                            Drives the <strong>Demos</strong> tab: auto-backup, the on-disk demo list, and YouTube renders.
                        </div>
                    </div>
                    <button class="btn-ghost" @click="pickDemos">Change</button>
                </div>
                <div class="text-sm text-neutral-300 break-all" :title="config.config.demos_path || ''">
                    {{ displayPath(config.config.demos_path) || '(not set)' }}
                </div>
                <div class="flex items-center justify-between gap-3 pt-2 border-t border-white/[0.05]">
                    <div>
                        <div class="text-sm font-medium">Include subfolders</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Watch nested folders too (e.g. <code class="bg-black/40 px-1 rounded">demos/2024/</code>).
                            Takes effect on next Start.
                        </div>
                    </div>
                    <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                        <input
                            type="checkbox"
                            class="sr-only peer"
                            :checked="config.config.include_subfolders"
                            @change="config.save({ include_subfolders: ($event.target as HTMLInputElement).checked })"
                        />
                        <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                        <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                    </label>
                </div>

                <!-- CPU throttle -->
                <div class="pt-3 border-t border-white/5">
                    <div class="mb-2">
                        <div class="text-sm font-medium">CPU usage during hashing</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            How much of one CPU core the launcher may use while hashing your demos.
                            Lower = more comfortable while gaming, slower rescans of big folders.
                            The <strong class="text-brand-400">Speed up</strong> button on the dashboard
                            temporarily overrides this for a backlog drain.
                        </div>
                    </div>
                    <div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
                        <button
                            v-for="opt in [
                                { label: 'Background', sub: '15%', value: 15 },
                                { label: 'Normal',     sub: '25%', value: 25 },
                                { label: 'Fast',       sub: '50%', value: 50 },
                                { label: 'No limit',   sub: 'max', value: 0  },
                            ]"
                            :key="opt.value"
                            class="px-3 py-2 rounded text-sm border transition-colors text-left"
                            :class="config.config.cpu_throttle_pct === opt.value
                                ? 'bg-brand-500/20 border-brand-500/60 text-brand-200'
                                : 'bg-white/5 border-white/10 hover:bg-white/10 text-neutral-300'"
                            @click="setThrottlePreference(opt.value)"
                        >
                            <div class="font-semibold">{{ opt.label }}</div>
                            <div class="text-xs text-neutral-500">{{ opt.sub }} CPU</div>
                        </button>
                    </div>
                    <p class="text-xs text-neutral-500 mt-2">
                        Takes effect immediately for the running watcher.
                    </p>
                </div>
            </section>

            <!-- Token -->
            <section
                ref="tokenSection"
                class="bg-neutral-900 border rounded-lg p-4 space-y-3 transition-all duration-500"
                :class="highlightToken
                    ? 'border-brand-500/70 ring-2 ring-brand-500/40 shadow-lg shadow-brand-500/10'
                    : 'border-white/10'"
            >
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Account token</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Personal access token from the
                            <a href="#" class="text-brand-400 hover:underline"
                               @click.prevent="openExternal('https://defrag.racing/user/settings?tab=security')">
                                "Launcher Tokens" block on defrag.racing → Settings → Security
                            </a>.
                            Stored in your OS keyring. Unlocks:
                        </div>
                        <ul class="text-xs text-neutral-400 mt-1 space-y-0.5 pl-1">
                            <TokenFeatureList />
                        </ul>
                    </div>
                </div>

                <div v-if="config.hasToken" class="flex items-center gap-2">
                    <div class="flex-1 text-sm text-emerald-400 font-mono">• • • • • • • • • • •  (stored)</div>
                    <button class="btn-ghost" @click="showTokenForm = !showTokenForm">Replace</button>
                    <button class="btn-danger" @click="clearToken">Clear</button>
                </div>
                <div v-else class="text-sm text-amber-300">
                    No token saved - the features above are disabled.
                    <div class="text-emerald-300 font-semibold mt-2">Works without a token:</div>
                    <ul class="text-xs text-emerald-200/90 mt-1 space-y-0.5 pl-1">
                        <TokenFreeFeatures />
                    </ul>
                </div>

                <div v-if="!config.hasToken || showTokenForm" class="flex gap-2">
                    <input
                        v-model="tokenInput"
                        type="text"
                        placeholder="Paste token here"
                        class="flex-1 bg-black/60 border border-white/10 rounded px-3 py-2 text-sm font-mono"
                    />
                    <button class="btn-primary" :disabled="!tokenInput.trim() || tokenSaving" @click="saveToken">
                        {{ tokenSaving ? 'Saving…' : 'Save' }}
                    </button>
                </div>
                <div v-if="tokenError" class="mt-2 rounded border border-red-500/40 bg-red-500/10 p-3 text-xs text-red-200 space-y-1.5">
                    <div class="flex items-start gap-2">
                        <span class="text-red-400 mt-0.5 flex-shrink-0">✕</span>
                        <span>{{ tokenError }}</span>
                    </div>
                    <div class="text-red-300/80 pl-6">
                        Create the token from the <strong class="text-red-200">Launcher Tokens</strong> block under
                        <span class="font-mono">defrag.racing &gt; Settings &gt; Security</span> - not another token type.
                    </div>
                </div>
            </section>

            <!-- Force re-check uploaded demos -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                <div>
                    <div class="font-semibold">Re-check uploaded demos</div>
                    <div class="text-xs text-neutral-500 mt-0.5">
                        Forget the local "already uploaded" cache. Next Start re-asks the server
                        for every demo - useful if a demo was deleted on defrag.racing and you want
                        to re-upload it.
                    </div>
                </div>
                <button class="btn-ghost flex-shrink-0" :disabled="reCheckBusy || reCheckCooldown > 0" @click="forceRecheck">
                    {{ reCheckBusy ? 'Re-checking…' : (reCheckCooldown > 0 ? `Started - wait ${reCheckCooldown}s` : 'Force re-check') }}
                </button>
            </section>

            <!-- Check & repair -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-center justify-between gap-3">
                    <div>
                        <div class="font-semibold">Check &amp; repair</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Scan the launcher's local state - login, demos folder, backup cache,
                            activity list, watcher - and fix anything corrupt. Your demos on the
                            server are never touched.
                        </div>
                    </div>
                    <button class="btn-ghost flex-shrink-0" :disabled="healthBusy" @click="runHealthCheck">
                        {{ healthBusy ? 'Checking…' : (healthRan ? 'Re-run' : 'Run check') }}
                    </button>
                </div>

                <ul v-if="healthRan" class="space-y-1.5 pt-1">
                    <li
                        v-for="item in healthItems"
                        :key="item.id"
                        class="flex items-start gap-3 text-sm border-t border-white/[0.05] pt-2 first:border-t-0 first:pt-0"
                    >
                        <span class="mt-1.5 w-2 h-2 rounded-full flex-shrink-0" :class="healthDot(item.status)"></span>
                        <div class="flex-1 min-w-0">
                            <div class="text-neutral-200 font-medium">{{ item.title }}</div>
                            <div class="text-xs text-neutral-500 break-words">{{ item.detail }}</div>
                        </div>
                        <button
                            v-if="item.fix"
                            class="btn-ghost flex-shrink-0 text-xs"
                            :disabled="healthFixing === item.id"
                            @click="runHealthRepair(item)"
                        >{{ healthFixing === item.id ? 'Fixing…' : 'Fix' }}</button>
                    </li>
                </ul>
            </section>

            <!-- Autostart -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                <div>
                    <div class="font-semibold">Start with system</div>
                    <div class="text-xs text-neutral-500 mt-0.5">
                        Launch silently to the tray on login so the demo watcher
                        and <code class="bg-black/40 px-1 rounded">defrag://</code> links
                        keep working without you having to open the launcher manually.
                    </div>
                </div>
                <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                    <input
                        type="checkbox"
                        class="sr-only peer"
                        :checked="autostart"
                        @change="toggleAutostart(($event.target as HTMLInputElement).checked)"
                    />
                    <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                    <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                </label>
            </section>

            <!-- Auto-update is intentionally not user-toggleable. Security
                 fixes (token leak protection, signed-update bypasses, MSI
                 cleanup bugs that wipe user data, etc.) have to reach
                 every install without depending on the user remembering to
                 check Releases. The config field still exists and defaults
                 to true; the toggle UI is preserved below in case we ever
                 want to add an "expert mode" escape hatch.
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                <div>
                    <div class="font-semibold">Automatic updates</div>
                    <div class="text-xs text-neutral-500 mt-0.5">
                        Checks `defrag.racing` and GitHub on startup for a newer signed release.
                        Off = check Releases manually.
                    </div>
                </div>
                <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                    <input
                        type="checkbox"
                        class="sr-only peer"
                        :checked="config.config.auto_update_enabled"
                        @change="config.save({ auto_update_enabled: ($event.target as HTMLInputElement).checked })"
                    />
                    <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                    <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                </label>
            </section>
            -->
            <!-- Auto-update status (read-only, informational) -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-center gap-2">
                    <span class="text-emerald-400">●</span>
                    <div class="font-semibold">Automatic updates: on</div>
                </div>
                <div class="text-xs text-neutral-500 leading-relaxed">
                    The launcher checks `defrag.racing` and GitHub for a newer signed release
                    on every startup. Required to keep security fixes flowing - cannot be
                    disabled. When an update is available an "Install &amp; restart" banner appears
                    on every tab - and right here, with the full changelog.
                </div>
                <!-- Manual check + next-check countdown. Lives here
                     (not on the main dashboard) because it's a setting-
                     adjacent diagnostic, not something the user needs to
                     see every time the launcher opens. -->
                <div class="flex items-center justify-between gap-3 pt-2 border-t border-white/[0.04]">
                    <div class="text-xs">
                        <span v-if="updater.state.kind === 'checking'" class="text-neutral-300">Checking…</span>
                        <span v-else-if="updater.upToDateToast" class="text-emerald-400">✓ You're on the latest version</span>
                        <span v-else-if="updater.state.kind === 'available'" class="text-brand-300">
                            Update v{{ updater.state.version }} is available.
                        </span>
                        <span v-else-if="updater.state.kind === 'error'" class="text-red-300">
                            Last check failed: {{ updater.state.message }}
                        </span>
                        <span v-else-if="countdownLabel" class="text-neutral-500">
                            Next check in <span class="font-mono text-neutral-300">{{ countdownLabel }}</span>
                        </span>
                        <span v-else class="text-neutral-500">Idle</span>
                    </div>
                    <button
                        class="btn-ghost text-xs disabled:opacity-50"
                        :disabled="updater.manualBusy"
                        @click="manualCheck"
                    >{{ updater.manualBusy ? 'Checking…' : 'Check now' }}</button>
                </div>

                <!-- The actionable banner (View changes + Install & restart,
                     with the inline changelog) right under Check now, so a
                     manual check that finds an update is self-contained here
                     instead of pointing the user back to the Dashboard.
                     Renders nothing when there's no update in flight. The
                     app-level copy is suppressed on this route so it doesn't
                     stack with this one. -->
                <div v-if="updater.state.kind === 'available' || updater.state.kind === 'downloading' || updater.state.kind === 'installing' || updater.state.kind === 'error'" class="-mx-4 -mb-4 mt-1 rounded-b-lg overflow-hidden">
                    <UpdateBanner />
                </div>
            </section>

            <!-- Developer mode. Toggle reveals the advanced launch surface:
                 custom args appended to Quick launch + named launch
                 profiles that show as extra launch buttons in the top nav. -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-center justify-between gap-3">
                    <div>
                        <div class="font-semibold flex items-center gap-2">
                            <span>🛠️</span><span>Developer mode</span>
                        </div>
                        <div class="text-xs text-neutral-500 mt-0.5 leading-relaxed">
                            Adds custom engine arguments and your own named Quick-launch
                            profiles. For power users tweaking startup flags - leave off if
                            you're not sure.
                        </div>
                    </div>
                    <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                        <input
                            type="checkbox"
                            class="sr-only peer"
                            :checked="config.config.developer_mode"
                            @change="toggleDeveloperMode(($event.target as HTMLInputElement).checked)"
                        />
                        <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                        <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                    </label>
                </div>

                <div v-if="config.config.developer_mode" class="space-y-4 pt-2 border-t border-white/[0.06]">
                    <!-- Custom args appended to the main Quick launch. -->
                    <div class="space-y-1.5">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Custom launch arguments</div>
                        <input
                            v-model="customArgs"
                            type="text"
                            spellcheck="false"
                            placeholder='e.g. +set fs_game defrag +set r_fullscreen 0'
                            class="w-full bg-black/60 border border-white/10 rounded px-3 py-2 text-sm font-mono text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                            @blur="saveCustomArgs"
                            @keydown.enter="saveCustomArgs"
                        />
                        <div class="text-[11px] text-neutral-500">
                            Appended to the <strong>Quick launch</strong> button. Quotes are respected,
                            so <span class="font-mono">"my mod"</span> stays one argument.
                        </div>
                    </div>

                    <!-- Named launch profiles. Each becomes its own button in
                         the top nav's launch menu. -->
                    <div class="space-y-2">
                        <div class="flex items-center justify-between">
                            <div class="text-xs uppercase tracking-wider text-neutral-500">Launch profiles</div>
                            <button class="btn-ghost text-xs" @click="addProfile">+ Add profile</button>
                        </div>
                        <p v-if="profiles.length === 0" class="text-[11px] text-neutral-500">
                            No profiles yet. Add one to get an extra labelled launch button
                            (e.g. "Fullscreen", "Mod X") next to Quick launch.
                        </p>
                        <div
                            v-for="p in profiles"
                            :key="p.id"
                            class="flex items-center gap-2"
                        >
                            <input
                                v-model="p.name"
                                type="text"
                                spellcheck="false"
                                placeholder="Name (e.g. Fullscreen)"
                                class="w-40 flex-shrink-0 bg-black/60 border border-white/10 rounded px-2 py-1.5 text-sm text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                                @blur="persistProfiles"
                            />
                            <input
                                v-model="p.args"
                                type="text"
                                spellcheck="false"
                                placeholder="Arguments (e.g. +set r_fullscreen 1)"
                                class="flex-1 min-w-0 bg-black/60 border border-white/10 rounded px-2 py-1.5 text-sm font-mono text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                                @blur="persistProfiles"
                                @keydown.enter="persistProfiles"
                            />
                            <button
                                class="flex-shrink-0 px-2 py-1.5 rounded bg-red-500/10 hover:bg-red-500/20 text-red-300 text-xs"
                                title="Remove profile"
                                @click="removeProfile(p.id)"
                            >Remove</button>
                        </div>
                        <p v-if="profiles.length > 0" class="text-[11px] text-neutral-500">
                            Each profile launches the engine with just its own arguments and
                            appears in the launch menu next to <strong>Quick launch</strong>.
                            Needs an engine set above.
                        </p>
                    </div>
                </div>
            </section>

            <!-- Reset - wipes every setting and token and drops the user
                 back into the onboarding wizard. Lives in a red-tinted card
                 so it reads as destructive at a glance. (Re-run setup used
                 to be a separate button; it's gone - Reset is the canonical
                 way to redo setup, and every field is editable above anyway.) -->
            <section class="bg-red-500/5 border border-red-500/30 rounded-lg p-4 flex items-center justify-between">
                <div>
                    <div class="font-semibold text-red-300">Reset launcher</div>
                    <div class="text-xs text-neutral-500 mt-0.5">Clear all settings and the stored token, then re-run the setup wizard. Demos on your PC are not touched.</div>
                </div>
                <button class="btn-danger" @click="showResetConfirm = true">Reset</button>
            </section>

            <div class="text-xs text-neutral-600 text-center pt-4">
                Defrag Racing Launcher v{{ appVersion || '…' }}
            </div>
        </div>

        <!-- Reset confirmation modal. Typed confirmation (not a native
             confirm) both because confirm() is unreliable in WebView2 and
             because a wipe-everything action deserves a deliberate step. -->
        <div
            v-if="showResetConfirm"
            class="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 p-4"
            @click.self="cancelReset"
        >
            <div class="bg-neutral-900 border border-red-500/40 rounded-lg p-5 max-w-md w-full space-y-3">
                <div class="font-semibold text-red-300 text-lg">Reset launcher?</div>
                <p class="text-sm text-neutral-300">
                    This clears <strong>everything the launcher stored</strong>: your account token,
                    the engine path, the demos-folder path, and all settings. You'll be taken back
                    through the setup wizard.
                </p>
                <p class="text-xs text-neutral-500">
                    Your demo files on your PC and your demos already backed up to defrag.racing are
                    <strong>not</strong> touched.
                </p>
                <div class="pt-1">
                    <label class="text-xs text-neutral-400">Type <code class="bg-black/40 px-1 rounded text-amber-300">yes</code> to confirm:</label>
                    <input
                        v-model="resetConfirmText"
                        type="text"
                        placeholder="yes"
                        autocomplete="off"
                        class="mt-1 w-full bg-black/40 border border-white/15 rounded px-3 py-2 text-sm text-neutral-100 focus:border-red-500/60 focus:outline-none"
                        @keyup.enter="resetLauncher"
                    />
                </div>
                <div class="flex justify-end gap-2 pt-1">
                    <button class="btn-ghost" @click="cancelReset">Cancel</button>
                    <button
                        class="btn-danger disabled:opacity-40 disabled:cursor-not-allowed"
                        :disabled="!resetConfirmValid || resetting"
                        @click="resetLauncher"
                    >{{ resetting ? 'Resetting…' : 'Reset everything' }}</button>
                </div>
            </div>
        </div>
    </div>
</template>

<style scoped>
.btn-primary {
    @apply px-3 py-1.5 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-400 text-sm font-semibold disabled:opacity-40 disabled:cursor-not-allowed;
}
.btn-ghost {
    @apply px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-sm;
}
.btn-danger {
    @apply px-3 py-1.5 rounded bg-red-500/15 hover:bg-red-500/25 text-red-300 text-sm;
}
</style>
