<script setup lang="ts">
    import { computed, onMounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { tauri, type EngineCandidate } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import TokenFeatureList from '../components/TokenFeatureList.vue';

    const router = useRouter();
    const config = useConfigStore();

    // 1 = intro, 2 = engine picker, 3 = token, 4 = finish.
    // Engine + demos come before the token now: they're the mandatory
    // base (without them the launcher can't open join links or back
    // anything up), so the user clears the must-haves first and only
    // then meets the optional token step.
    const step = ref<1 | 2 | 3 | 4>(1);

    // --- step 3: token --------------------------------------------------
    const token = ref('');
    const tokenSaving = ref(false);
    const tokenError = ref<string | null>(null);
    const tokenSkipped = ref(false);
    // Guard so a token can't be skipped by accident - the button opens a
    // confirmation that spells out exactly which features go dark.
    const showSkipConfirm = ref(false);

    const saveToken = async () => {
        tokenError.value = null;
        if (! token.value.trim()) return;
        tokenSaving.value = true;
        try {
            await tauri.saveToken(token.value.trim());
            token.value = '';
            tokenSkipped.value = false;
            await config.refresh();
            step.value = 4;
        } catch (e: any) {
            tokenError.value = e.toString();
        } finally {
            tokenSaving.value = false;
        }
    };

    // Skip is a two-step action: the button only opens the warning
    // dialog, and the user has to confirm there to actually proceed
    // token-less.
    const requestSkipToken = () => {
        showSkipConfirm.value = true;
    };
    const confirmSkipToken = () => {
        tokenSkipped.value = true;
        showSkipConfirm.value = false;
        step.value = 4;
    };

    const openTokensPage = () =>
        openUrl('https://defrag.racing/user/settings?tab=security');

    // --- step 2: engine + demos -----------------------------------------
    const engines = ref<EngineCandidate[]>([]);
    const enginesLoading = ref(false);
    const selectedEngine = ref<string | null>(null);
    const demosPath = ref<string | null>(null);

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

    // Engine + demos folder are mandatory - without an engine even
    // defrag:// join links can't open, and without a demos folder the
    // whole point of the launcher (backup) is dead. So no skip here; the
    // token (step 2) is the only optional part of setup.
    const canProceedFromEngine = computed(() => {
        return !! selectedEngine.value && !! demosPath.value;
    });

    // --- finish ---------------------------------------------------------
    const finishing = ref(false);

    const finish = async () => {
        finishing.value = true;
        try {
            // Auto-upload stays OFF until the user clicks Start on the
            // dashboard. Earlier this fired up the watcher right after
            // onboarding if both token and demos were set ("one fewer
            // click for the happy path") - but that flat-out contradicted
            // the step-2 promise of "off by default", and silently
            // starting to hash + upload the user's entire demo folder
            // the second they paste a token was the right call to
            // reverse. The dashboard banner explains it and the Start
            // button is impossible to miss.
            await config.save({
                engine_path: selectedEngine.value,
                demos_path: demosPath.value,
                auto_upload_enabled: false,
                onboarding_completed: true,
            });
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
                        A small companion app for
                        <a href="#" @click.prevent="openUrl('https://defrag.racing')" class="text-brand-400 hover:underline">defrag.racing</a>.
                        Here's what's inside:
                    </p>
                    <ul class="text-sm text-neutral-300 space-y-1.5 pl-1">
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Auto-backup demos</strong> - every new <code class="text-xs bg-black/40 px-1 rounded">.dm_*</code> file in your demos folder gets uploaded to your account.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Demos</strong> - browse every local demo and queue YouTube renders from one click, right next to the live auto-backup status.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Servers</strong> - live list of Defrag servers with your PB and rank on each map.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Records &amp; Maps</strong> - paginated leaderboards (VQ3 + CPM side-by-side) and the full map list with thumbnails.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Notifications</strong> - PB beats, world record takes, render-done events and account alerts, sorted by type.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>History</strong> - log of every <code class="text-xs bg-black/40 px-1 rounded">defrag://</code> server you joined, one-click Reconnect.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong><code class="text-xs bg-black/40 px-1 rounded">defrag://</code> links</strong> - click "Join" on the website, the launcher asks you to confirm, your engine launches.</span>
                        </li>
                    </ul>
                    <p class="text-xs text-neutral-500 leading-relaxed pt-1">
                        Most of this needs a token from your defrag.racing account. The <code class="bg-black/40 px-1 rounded">defrag://</code> handler works without one. Setup takes under a minute.
                    </p>
                    <div class="flex justify-end pt-2">
                        <button class="btn-primary" @click="step = 2">Next</button>
                    </div>
                </div>

                <!-- step 3: token (optional - last because it's the only
                     skippable part of setup) -->
                <div v-else-if="step === 3" class="space-y-4">
                    <h2 class="text-xl font-bold">Account token <span class="text-sm font-normal text-neutral-500">(optional)</span></h2>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        A token links the launcher to your defrag.racing account. It unlocks:
                    </p>
                    <ul class="text-sm text-neutral-300 space-y-1 pl-1">
                        <li class="flex gap-2"><span class="text-brand-400 mt-0.5">✓</span><span>Server browser with your personal best / rank per map</span></li>
                        <li class="flex gap-2"><span class="text-brand-400 mt-0.5">✓</span><span>Notifications - record + system alerts for your account</span></li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">✓</span>
                            <span>
                                Optional auto-upload of new demos
                                <span class="text-neutral-500">(off by default - you turn it on later with the Start button)</span>
                            </span>
                        </li>
                    </ul>
                    <p class="text-xs text-neutral-500 leading-relaxed">
                        Without a token, only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links work. You can paste a token later from Settings.
                    </p>

                    <!-- Big obvious step-1 CTA. Was "blue underlined link"
                         before and users overlooked it; making it a chunky
                         primary-style button removes the "where do I get
                         the token?" friction. -->
                    <div class="pt-1">
                        <button
                            type="button"
                            class="group w-full flex items-center justify-between gap-3 px-4 py-3 rounded-lg bg-brand-500/25 hover:bg-brand-500/40 border border-brand-400/60 hover:border-brand-300 cursor-pointer transition shadow-sm"
                            @click="openTokensPage"
                        >
                            <div class="text-left">
                                <div class="text-sm font-semibold text-brand-100 group-hover:underline">
                                    Step 1 - Click here to open the token page
                                </div>
                                <div class="text-xs text-brand-300/70 mt-0.5 font-mono">
                                    defrag.racing &gt; Settings &gt; Security &gt; "Launcher Tokens"
                                </div>
                            </div>
                            <span class="flex items-center gap-1 text-brand-200 font-semibold text-sm flex-shrink-0 whitespace-nowrap">
                                Open <span class="text-lg">↗</span>
                            </span>
                        </button>
                        <div class="text-[11px] text-neutral-500 mt-1 text-center">opens in your web browser</div>
                    </div>

                    <!-- Ordered list starts at 2 because step 1 is the
                         big CTA button above. Custom marker color keeps
                         the numbers from going default-black on dark bg. -->
                    <ol start="2" class="text-sm text-neutral-300 list-decimal list-inside space-y-1 pt-1 pl-1 marker:text-brand-400">
                        <li>On that page, find the <strong class="text-brand-200">Launcher Tokens</strong> block, click <em>New token</em> and label it (e.g. "Home PC")</li>
                        <li>Copy the generated token and paste it below</li>
                    </ol>

                    <input
                        v-model="token"
                        type="text"
                        placeholder="1|abc123def…  (paste token here)"
                        class="w-full bg-black/60 border border-white/10 rounded px-3 py-2 text-sm font-mono focus:border-brand-500/60 focus:outline-none"
                        @keydown.enter="saveToken"
                    />
                    <p v-if="tokenError" class="text-xs text-red-400">{{ tokenError }}</p>

                    <div class="flex justify-between pt-2">
                        <button class="btn-ghost" @click="requestSkipToken">Skip - defrag:// only</button>
                        <button class="btn-primary" :disabled="!token.trim() || tokenSaving" @click="saveToken">
                            {{ tokenSaving ? 'Saving…' : 'Save & continue' }}
                        </button>
                    </div>

                    <!-- Skip confirmation. Pasting a token is the single
                         biggest "did I set this up right?" moment, so
                         skipping it can't be a one-tap accident: this
                         dialog names every feature that stays dark and
                         makes the user actively choose the crippled mode. -->
                    <div
                        v-if="showSkipConfirm"
                        class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
                        @click.self="showSkipConfirm = false"
                    >
                        <div class="max-w-md w-full bg-neutral-900 border border-amber-500/40 rounded-xl p-5 space-y-3">
                            <h3 class="text-lg font-bold text-amber-200">Continue without a token?</h3>
                            <p class="text-sm text-neutral-300">
                                Without a token the launcher runs in <strong>defrag:// only</strong> mode. These features will be <strong>disabled</strong> and visibly empty:
                            </p>
                            <ul class="text-xs text-amber-100 space-y-0.5 pl-1 rounded border border-amber-500/30 bg-amber-500/10 p-3">
                                <TokenFeatureList />
                            </ul>
                            <p class="text-xs text-neutral-500">
                                Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links will work. You can paste a token anytime later from Settings.
                            </p>
                            <div class="flex justify-end gap-2 pt-1">
                                <button class="btn-ghost" @click="showSkipConfirm = false">Back - I'll add a token</button>
                                <button
                                    class="px-3 py-1.5 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-200 text-sm font-semibold"
                                    @click="confirmSkipToken"
                                >Skip anyway</button>
                            </div>
                        </div>
                    </div>
                </div>

                <!-- step 2: engine + demos (required - the launcher's
                     base, comes before the optional token step) -->
                <div v-else-if="step === 2" class="space-y-4">
                    <h2 class="text-xl font-bold">Defrag installation</h2>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        Pick the engine you want <code class="text-xs bg-black/40 px-1 rounded">defrag://</code> links to open, and confirm the demos folder the launcher will watch.
                        Both are required - the engine opens join links and the demos folder is what gets backed up.
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

                    <div v-if="selectedEngine" class="space-y-2 pt-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Selected engine</div>
                        <div class="flex items-center gap-2 bg-emerald-500/5 border border-emerald-500/30 rounded p-3">
                            <svg class="w-4 h-4 text-emerald-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                            </svg>
                            <div class="flex-1 text-sm text-neutral-200 break-all min-w-0">
                                {{ selectedEngine }}
                            </div>
                            <button class="btn-ghost flex-shrink-0" @click="selectedEngine = null">Change</button>
                        </div>
                    </div>

                    <div v-if="selectedEngine || demosPath" class="space-y-2 pt-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Demos folder</div>
                        <div class="flex items-center gap-2 bg-black/30 border border-white/10 rounded p-3">
                            <div class="flex-1 text-sm text-neutral-200 break-all min-w-0">
                                {{ demosPath || '(not detected - click Change)' }}
                            </div>
                            <button class="btn-ghost" @click="pickDemosFolder">Change</button>
                        </div>
                    </div>

                    <div class="pt-2 space-y-2">
                        <button class="btn-primary w-full" :disabled="!canProceedFromEngine" @click="step = 3">Next</button>
                        <p v-if="!canProceedFromEngine" class="text-xs text-amber-300/80 text-center">
                            Pick your engine and demos folder to continue - both are required for the launcher (and even <code class="bg-black/40 px-1 rounded">defrag://</code> links) to work.
                        </p>
                    </div>
                </div>

                <!-- step 4: finish -->
                <div v-else class="space-y-4">
                    <h2 class="text-xl font-bold">All set</h2>
                    <ul class="text-sm text-neutral-300 space-y-1">
                        <li class="flex items-center gap-2">
                            <span :class="tokenSkipped ? 'text-amber-400' : 'text-brand-400'">{{ tokenSkipped ? '!' : '✓' }}</span>
                            <span v-if="tokenSkipped">Token skipped</span>
                            <span v-else>Token stored</span>
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span>Engine: {{ selectedEngine }}</span>
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span>Demos folder: {{ demosPath }}</span>
                        </li>
                    </ul>

                    <!-- Explicit warning when token was skipped. Without
                         this the user reaches the dashboard, finds it
                         mostly empty, and assumes the launcher is broken.
                         Listing the disabled features by name + a one-
                         click "go back" makes the trade-off legible. -->
                    <div
                        v-if="tokenSkipped"
                        class="rounded border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-100 space-y-1.5"
                    >
                        <div class="font-semibold text-amber-200">Without a token, these features stay disabled:</div>
                        <ul class="space-y-0.5 pl-1">
                            <TokenFeatureList />
                        </ul>
                        <div class="pt-1">
                            Only <code class="bg-black/40 px-1 rounded">defrag://</code> server-join links will work.
                            You can add a token anytime from
                            <strong class="text-amber-200">Settings → Auto-upload token</strong>.
                        </div>
                        <button class="mt-1 text-amber-200 hover:underline font-semibold" @click="step = 3">
                            ← Go back and paste a token instead
                        </button>
                    </div>

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
