<script setup lang="ts">
    import { computed, onMounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { tauri, type EngineCandidate } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const config = useConfigStore();

    // 1 = intro, 2 = token, 3 = engine picker, 4 = finish
    const step = ref<1 | 2 | 3 | 4>(1);

    // --- step 2: token --------------------------------------------------
    const token = ref('');
    const tokenSaving = ref(false);
    const tokenError = ref<string | null>(null);
    const tokenSkipped = ref(false);

    const saveToken = async () => {
        tokenError.value = null;
        if (! token.value.trim()) return;
        tokenSaving.value = true;
        try {
            await tauri.saveToken(token.value.trim());
            token.value = '';
            tokenSkipped.value = false;
            await config.refresh();
            step.value = 3;
        } catch (e: any) {
            tokenError.value = e.toString();
        } finally {
            tokenSaving.value = false;
        }
    };

    const skipToken = () => {
        tokenSkipped.value = true;
        step.value = 3;
    };

    const openTokensPage = () =>
        openUrl('https://defrag.racing/user/settings?tab=security');

    // --- step 3: engine + demos -----------------------------------------
    const engines = ref<EngineCandidate[]>([]);
    const enginesLoading = ref(false);
    const selectedEngine = ref<string | null>(null);
    const demosPath = ref<string | null>(null);
    const engineSkipped = ref(false);

    const rescanEngines = async () => {
        enginesLoading.value = true;
        try {
            engines.value = await tauri.detectEngines();
        } finally {
            enginesLoading.value = false;
        }
    };

    const pickManualEngine = async () => {
        const picked = await openDialog({
            title: 'Select oDFe / iDFe executable',
            multiple: false,
            directory: false,
        });
        if (typeof picked === 'string') {
            selectedEngine.value = picked;
            demosPath.value = await tauri.guessDemosPath(picked);
        }
    };

    const pickDemosFolder = async () => {
        const picked = await openDialog({
            title: 'Select your Defrag demos folder',
            multiple: false,
            directory: true,
        });
        if (typeof picked === 'string') {
            demosPath.value = picked;
        }
    };

    const selectEngine = async (path: string) => {
        selectedEngine.value = path;
        demosPath.value = await tauri.guessDemosPath(path);
    };

    const skipEngineSetup = () => {
        engineSkipped.value = true;
        step.value = 4;
    };

    const canProceedFromEngine = computed(() => {
        if (engineSkipped.value) return true;
        return !! demosPath.value;
    });

    // --- finish ---------------------------------------------------------
    const finishing = ref(false);

    const finish = async () => {
        finishing.value = true;
        try {
            await config.save({
                engine_path: selectedEngine.value,
                demos_path: demosPath.value,
                auto_upload_enabled: !! (demosPath.value && ! tokenSkipped.value),
                onboarding_completed: true,
            });
            // If the user supplied both a token and demos path, fire up the
            // watcher right away — one fewer click for the happy path.
            if (config.hasToken && config.config.demos_path) {
                try {
                    await tauri.startAutoUpload();
                    await config.refresh();
                } catch { /* user can enable manually on dashboard */ }
            }
            router.replace({ name: 'dashboard' });
        } finally {
            finishing.value = false;
        }
    };

    onMounted(() => {
        // Prefetch engine list so step 3 shows results instantly. If the
        // user is skipping straight through, this cost is wasted but it's
        // tiny (<100ms on a warm filesystem).
        rescanEngines();
    });
</script>

<template>
    <div class="min-h-full flex items-center justify-center p-6">
        <div class="max-w-xl w-full bg-neutral-900 border border-white/10 rounded-xl overflow-hidden">
            <!-- progress -->
            <div class="h-1 bg-white/5">
                <div class="h-full bg-brand-500 transition-all" :style="{ width: (step * 25) + '%' }"></div>
            </div>

            <div class="p-6">
                <!-- step 1 -->
                <div v-if="step === 1" class="space-y-4">
                    <h1 class="text-2xl font-bold">Welcome to Defrag Racing Launcher</h1>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        This little app does two things: it can
                        <strong class="text-neutral-200">automatically back up</strong> every demo you record to your
                        <a href="#" @click.prevent="openUrl('https://defrag.racing')" class="text-brand-400 hover:underline">defrag.racing</a>
                        account, and it opens
                        <code class="text-xs bg-black/40 px-1 rounded">defrag://</code>
                        links from the website directly in your engine of choice.
                    </p>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        Setup takes under a minute. You can skip the auto-upload if you only want the
                        <code class="text-xs bg-black/40 px-1 rounded">defrag://</code>
                        link handler.
                    </p>
                    <div class="flex justify-end pt-2">
                        <button class="btn-primary" @click="step = 2">Next</button>
                    </div>
                </div>

                <!-- step 2: token -->
                <div v-else-if="step === 2" class="space-y-4">
                    <h2 class="text-xl font-bold">Auto-upload token</h2>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        To automatically back up demos, paste a personal access token from your defrag.racing account.
                        Without a token the launcher still works for
                        <code class="text-xs bg-black/40 px-1 rounded">defrag://</code> links.
                    </p>

                    <ol class="text-sm text-neutral-300 list-decimal list-inside space-y-1">
                        <li>
                            <button class="text-brand-400 hover:underline" @click="openTokensPage">
                                Open defrag.racing → Settings → Security
                            </button>
                        </li>
                        <li>Under <em>Launcher tokens</em>, click <em>New token</em> and give it a label (e.g. "Home PC")</li>
                        <li>Copy the generated token and paste it below</li>
                    </ol>

                    <input
                        v-model="token"
                        type="text"
                        placeholder="1|abc123def…"
                        class="w-full bg-black/60 border border-white/10 rounded px-3 py-2 text-sm font-mono"
                        @keydown.enter="saveToken"
                    />
                    <p v-if="tokenError" class="text-xs text-red-400">{{ tokenError }}</p>

                    <div class="flex justify-between pt-2">
                        <button class="btn-ghost" @click="skipToken">Skip (no auto-upload)</button>
                        <button class="btn-primary" :disabled="!token.trim() || tokenSaving" @click="saveToken">
                            {{ tokenSaving ? 'Saving…' : 'Save & continue' }}
                        </button>
                    </div>
                </div>

                <!-- step 3: engine + demos -->
                <div v-else-if="step === 3" class="space-y-4">
                    <h2 class="text-xl font-bold">Defrag installation</h2>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        Pick the engine you want <code class="text-xs bg-black/40 px-1 rounded">defrag://</code> links to open, and confirm the demos folder the launcher will watch.
                        Both are optional — skip if you only want the token for manual use.
                    </p>

                    <div class="space-y-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Engines</div>
                        <div v-if="enginesLoading" class="text-sm text-neutral-500">Scanning…</div>
                        <div v-else-if="!engines.length" class="text-sm text-neutral-500">
                            None detected automatically.
                        </div>
                        <label
                            v-for="e in engines"
                            :key="e.path"
                            class="flex items-start gap-2 cursor-pointer bg-black/30 border border-white/10 rounded p-3 hover:bg-black/40"
                            :class="{ 'ring-1 ring-brand-500': selectedEngine === e.path }"
                        >
                            <input
                                type="radio"
                                name="engine"
                                :value="e.path"
                                :checked="selectedEngine === e.path"
                                @change="selectEngine(e.path)"
                                class="mt-1"
                            />
                            <div class="min-w-0">
                                <div class="font-semibold uppercase text-xs text-brand-400">{{ e.kind }}</div>
                                <div class="text-sm text-neutral-200 break-all">{{ e.path }}</div>
                            </div>
                        </label>

                        <button class="btn-ghost w-full" @click="pickManualEngine">Browse manually…</button>
                        <button class="text-xs text-neutral-500 hover:text-neutral-300" @click="rescanEngines">Rescan</button>
                    </div>

                    <div v-if="selectedEngine || demosPath" class="space-y-2 pt-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Demos folder</div>
                        <div class="flex items-center gap-2 bg-black/30 border border-white/10 rounded p-3">
                            <div class="flex-1 text-sm text-neutral-200 break-all min-w-0">
                                {{ demosPath || '(not detected — click Change)' }}
                            </div>
                            <button class="btn-ghost" @click="pickDemosFolder">Change</button>
                        </div>
                    </div>

                    <div class="flex justify-between pt-2">
                        <button class="btn-ghost" @click="skipEngineSetup">Skip</button>
                        <button class="btn-primary" :disabled="!canProceedFromEngine" @click="step = 4">Next</button>
                    </div>
                </div>

                <!-- step 4: finish -->
                <div v-else class="space-y-4">
                    <h2 class="text-xl font-bold">All set</h2>
                    <ul class="text-sm text-neutral-300 space-y-1">
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span v-if="tokenSkipped">Token skipped — auto-upload disabled</span>
                            <span v-else>Token stored in your OS keyring</span>
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span v-if="engineSkipped">Engine + demos folder skipped</span>
                            <span v-else-if="selectedEngine">Engine: {{ selectedEngine }}</span>
                            <span v-else>No engine selected</span>
                        </li>
                        <li v-if="demosPath" class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span>Demos folder: {{ demosPath }}</span>
                        </li>
                    </ul>
                    <div class="flex justify-end pt-2">
                        <button class="btn-primary" :disabled="finishing" @click="finish">
                            {{ finishing ? 'Finishing…' : 'Open launcher' }}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    </div>
</template>

<style scoped>
.btn-primary {
    @apply px-4 py-2 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-400 text-sm font-semibold disabled:opacity-40 disabled:cursor-not-allowed;
}
.btn-ghost {
    @apply px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-neutral-300 text-sm;
}
</style>
