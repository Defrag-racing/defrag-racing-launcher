<script setup lang="ts">
    import { onMounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { tauri, type EngineCandidate } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const config = useConfigStore();

    const engines = ref<EngineCandidate[]>([]);
    const tokenInput = ref('');
    const tokenSaving = ref(false);
    const tokenError = ref<string | null>(null);
    const showTokenForm = ref(false);

    const appVersion = ref('');
    const reCheckBusy = ref(false);
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
        if (! confirm('Forget which demos have been uploaded? Next Start will re-hash and re-check every demo with the server.')) return;
        reCheckBusy.value = true;
        try {
            await tauri.clearUploadCache();
        } finally {
            reCheckBusy.value = false;
        }
    };

    const resetLauncher = async () => {
        if (! confirm('Clear all launcher settings and the stored token? This cannot be undone. Demos on your PC are not affected.')) return;
        await tauri.resetLauncher();
        await config.refresh();
        router.replace({ name: 'onboarding' });
    };
</script>

<template>
    <div class="flex-1 flex flex-col">
        <header class="px-5 py-3 border-b border-white/10 flex items-center gap-3">
            <button class="text-sm text-neutral-400 hover:text-neutral-200" @click="router.back()">← Back</button>
            <h1 class="font-semibold">Settings</h1>
        </header>

        <div class="flex-1 overflow-auto p-5 space-y-4 max-w-2xl w-full">
            <!-- Engine -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-2">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Defrag engine</div>
                        <div class="text-xs text-neutral-500 mt-0.5">Used when opening <code class="bg-black/40 px-1 rounded">defrag://</code> links.</div>
                    </div>
                    <button class="btn-ghost" @click="pickEngine">Change</button>
                </div>
                <div class="text-sm text-neutral-300 break-all">
                    {{ config.config.engine_path || '(not set)' }}
                </div>
            </section>

            <!-- Demos path -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Demos folder</div>
                        <div class="text-xs text-neutral-500 mt-0.5">The launcher watches this folder for new demos.</div>
                    </div>
                    <button class="btn-ghost" @click="pickDemos">Change</button>
                </div>
                <div class="text-sm text-neutral-300 break-all">
                    {{ config.config.demos_path || '(not set)' }}
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
                            <li>• Auto-backup of recorded demos</li>
                            <li>• Server browser with your PB / rank per map</li>
                            <li>• Record + system notifications from your account</li>
                        </ul>
                    </div>
                </div>

                <div v-if="config.hasToken" class="flex items-center gap-2">
                    <div class="flex-1 text-sm text-emerald-400 font-mono">• • • • • • • • • • •  (stored)</div>
                    <button class="btn-ghost" @click="showTokenForm = !showTokenForm">Replace</button>
                    <button class="btn-danger" @click="clearToken">Clear</button>
                </div>
                <div v-else class="text-sm text-amber-300">
                    No token saved - all three features above are disabled. Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links work.
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
                <button class="btn-ghost flex-shrink-0" :disabled="reCheckBusy" @click="forceRecheck">
                    {{ reCheckBusy ? '…' : 'Force re-check' }}
                </button>
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

            <!-- Auto-update -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                <div>
                    <div class="font-semibold">Automatic updates</div>
                    <div class="text-xs text-neutral-500 mt-0.5">
                        Checks <code class="bg-black/40 px-1 rounded">defrag.racing</code> and GitHub
                        on startup for a newer signed release. Off = check Releases manually.
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
