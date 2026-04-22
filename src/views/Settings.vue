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

    onMounted(async () => {
        engines.value = await tauri.detectEngines();
    });

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
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-2">
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
            </section>

            <!-- Token -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-start justify-between gap-3">
                    <div>
                        <div class="font-semibold">Auto-upload token</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            Personal access token from
                            <a href="#" class="text-brand-400 hover:underline"
                               @click.prevent="openUrl('https://defrag.racing/user/settings?tab=security')">
                                defrag.racing → Settings → Security
                            </a>.
                            Stored in your OS keyring.
                        </div>
                    </div>
                </div>

                <div v-if="config.hasToken" class="flex items-center gap-2">
                    <div class="flex-1 text-sm text-emerald-400 font-mono">• • • • • • • • • • •  (stored)</div>
                    <button class="btn-ghost" @click="showTokenForm = !showTokenForm">Replace</button>
                    <button class="btn-danger" @click="clearToken">Clear</button>
                </div>
                <div v-else class="text-sm text-neutral-500">No token saved — auto-upload disabled.</div>

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

            <!-- Run setup again -->
            <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between">
                <div>
                    <div class="font-semibold">Re-run setup</div>
                    <div class="text-xs text-neutral-500 mt-0.5">Go through the onboarding wizard again.</div>
                </div>
                <button class="btn-ghost" @click="runOnboarding">Run</button>
            </section>

            <!-- Reset — wipes every setting and token so the user can start
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
                Defrag Racing Launcher v{{ '0.1.2' }}
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
