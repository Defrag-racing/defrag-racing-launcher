<script setup lang="ts">
    import { computed, onActivated, onMounted, onUnmounted, ref } from 'vue';
    import { useRoute, useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { tauri, type EngineCandidate, type HealthItem } from '../lib/tauri';
    import TokenFeatureList from '../components/TokenFeatureList.vue';
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
    const demosSection = ref<HTMLElement | null>(null);
    let highlightTimer: number | undefined;
    onActivated(async () => {
        if (route.query.highlight !== 'demos') return;
        highlightDemos.value = true;
        router.replace({ name: 'settings', query: {} });
        await new Promise((r) => requestAnimationFrame(() => r(null)));
        demosSection.value?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        if (highlightTimer !== undefined) window.clearTimeout(highlightTimer);
        highlightTimer = window.setTimeout(() => { highlightDemos.value = false; }, 2600);
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
        engines.value = await tauri.detectEngines();
        appVersion.value = await tauri.appVersion();
        // Read the OS-level autostart state, not just our config -
        // catches the case where the user removed the registration
        // manually (Task Manager → Startup) outside the launcher.
        autostart.value = await tauri.isAutostartEnabled();
    });

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

    const runOnboarding = () => router.push({ name: 'onboarding' });

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

    const resetLauncher = async () => {
        if (! confirm('Clear all launcher settings and the stored token? This cannot be undone. Demos on your PC are not affected.')) return;
        await tauri.resetLauncher();
        await config.refresh();
        router.replace({ name: 'onboarding' });
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
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Account token</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Personal access token from
                            <a href="#" class="text-brand-400 hover:underline"
                               @click.prevent="openUrl('https://defrag.racing/user/settings?tab=security')">
                                defrag.racing → Settings → Security
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
                    No token saved - the features above are disabled. Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links work.
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
                <p v-if="tokenError" class="text-xs text-red-400">{{ tokenError }}</p>
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
                    disabled. When an update is available the dashboard shows an "Install &amp; restart"
                    banner.
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
                            Update v{{ updater.state.version }} is available - see Dashboard.
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
            </section>

            <!-- Run setup again -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between">
                <div>
                    <div class="font-semibold">Re-run setup</div>
                    <div class="text-xs text-neutral-500 mt-0.5">Go through the onboarding wizard again.</div>
                </div>
                <button class="btn-ghost" @click="runOnboarding">Run</button>
            </section>

            <!-- Reset - wipes every setting and token so the user can start
                 fresh without uninstalling. Lives in a red-tinted card so
                 it reads as destructive at a glance. -->
            <section class="bg-red-500/5 border border-red-500/30 rounded-lg p-4 flex items-center justify-between">
                <div>
                    <div class="font-semibold text-red-300">Reset launcher</div>
                    <div class="text-xs text-neutral-500 mt-0.5">Clear all settings and the stored token. Demos on your PC are not touched.</div>
                </div>
                <button class="btn-danger" @click="resetLauncher">Reset</button>
            </section>

            <div class="text-xs text-neutral-600 text-center pt-4">
                Defrag Racing Launcher v{{ appVersion || '…' }}
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
