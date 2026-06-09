<script setup lang="ts">
    import { computed, onActivated, onMounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openExternal } from '../lib/open';
    import { tauri, type EngineCandidate } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import TokenFeatureList from '../components/TokenFeatureList.vue';
    import TokenFreeFeatures from '../components/TokenFreeFeatures.vue';

    const router = useRouter();
    const config = useConfigStore();

    // 1 = intro, 2 = engine picker, 3 = token, 4 = finish.
    // Engine + demos come before the token now: they're the mandatory
    // base (without them the launcher can't open join links or back
    // anything up), so the user clears the must-haves first and only
    // then meets the optional token step.
    const step = ref<1 | 2 | 3 | 4>(1);

    // Step back one in the wizard (footer Back button). Token-skip state is
    // cleared so revisiting the token step is a clean choice again.
    const goBack = () => {
        if (step.value <= 1) return;
        step.value = (step.value - 1) as 1 | 2 | 3 | 4;
    };

    // --- step 3: token --------------------------------------------------
    const token = ref('');
    const tokenSaving = ref(false);
    const tokenError = ref<string | null>(null);
    const tokenSkipped = ref(false);
    // Account name the validated token resolved to, shown on the final
    // screen so the user can confirm they signed in as the right player.
    const tokenSavedName = ref<string | null>(null);
    // Guard so a token can't be skipped by accident - the button opens a
    // confirmation that spells out exactly which features go dark.
    const showSkipConfirm = ref(false);

    const saveToken = async () => {
        tokenError.value = null;
        if (! token.value.trim()) return;
        tokenSaving.value = true;
        try {
            // Validate against the server BEFORE saving. Pasting a token
            // that silently doesn't work - the classic "I made the wrong
            // token" - used to surface only hours later as a cryptic
            // rejection on the Servers tab. Catch it here with a message
            // that says what's wrong and where to fix it.
            const check = await tauri.validateToken(token.value.trim());
            if (! check.ok) {
                tokenError.value = check.message;
                return;
            }
            await tauri.saveToken(token.value.trim());
            tokenSavedName.value = check.name;
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
        openExternal('https://defrag.racing/user/settings?tab=security');

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

    // Windows long-path prefixes (\\?\ and \\?\UNC\) are an implementation
    // detail that scares non-technical users - strip them for display only.
    // The real value handed to the backend keeps the prefix untouched.
    const prettyPath = (p: string | null | undefined): string => {
        if (!p) return '';
        if (p.startsWith('\\\\?\\UNC\\')) return '\\\\' + p.slice(8);
        if (p.startsWith('\\\\?\\')) return p.slice(4);
        return p;
    };
    const pathFile = (p: string | null | undefined): string => {
        const s = prettyPath(p);
        const i = Math.max(s.lastIndexOf('\\'), s.lastIndexOf('/'));
        return i >= 0 ? s.slice(i + 1) : s;
    };
    const pathDir = (p: string | null | undefined): string => {
        const s = prettyPath(p);
        const i = Math.max(s.lastIndexOf('\\'), s.lastIndexOf('/'));
        return i >= 0 ? s.slice(0, i) : '';
    };

    const demosError = ref('');
    const pickDemosFolder = async () => {
        const picked = await openDialog({
            title: 'Select your Defrag demos folder',
            multiple: false,
            directory: true,
            // Open at the detected demos folder so the user lands in the right
            // place and just confirms (or steps into a subfolder).
            defaultPath: demosPath.value || undefined,
        });
        if (typeof picked !== 'string') return;
        // Must be the engine's demos folder or a subfolder of it - the demo
        // player / defrag:// handling derive the install + game from this path,
        // so a folder outside the engine's demos tree silently breaks them.
        if (selectedEngine.value) {
            try {
                await tauri.validateDemosPath(selectedEngine.value, picked);
            } catch (e: any) {
                demosError.value = typeof e === 'string' ? e : (e?.toString?.() ?? 'That folder is not inside your Defrag demos folder.');
                return;
            }
        }
        demosError.value = '';
        demosPath.value = picked;
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
            // Turn on launch-at-login by default. The launcher is a
            // background companion: the demo watcher and - especially -
            // the defrag:// join handler are only useful if it's actually
            // running, and a fresh user shouldn't have to discover the
            // Settings toggle to get "click Join on the site -> it
            // connects" to work. It starts hidden in the tray (HIDDEN_FLAG),
            // so this is invisible until needed, and the same Settings
            // toggle turns it back off for anyone who'd rather opt out.
            // Best-effort: a failure here must not block finishing setup.
            try {
                await tauri.setAutostartEnabled(true);
            } catch { /* non-fatal - user can toggle it in Settings */ }
            router.replace({ name: 'dashboard' });
        } finally {
            finishing.value = false;
        }
    };

    // Put the wizard back to a clean step 1. The view is kept-alive, so after
    // a Reset (which routes here from Settings) the cached instance would
    // otherwise still show the old "All set" finish step with the previous
    // engine/demos filled in. Resetting on entry makes Reset a true restart.
    const resetWizard = () => {
        step.value = 1;
        token.value = '';
        tokenError.value = null;
        tokenSkipped.value = false;
        tokenSavedName.value = null;
        showSkipConfirm.value = false;
        selectedEngine.value = null;
        demosPath.value = null;
        // Prefetch engine list so the picker shows results instantly.
        rescanEngines();
    };

    onMounted(resetWizard);
    onActivated(resetWizard);
</script>

<template>
    <div class="min-h-full flex items-center justify-center p-6 overflow-y-auto">
        <div class="max-w-5xl w-full bg-neutral-900 border border-white/10 rounded-xl overflow-hidden flex flex-col max-h-[calc(100vh-3rem)]">
            <!-- progress -->
            <div class="h-1 bg-white/5 flex-shrink-0">
                <div class="h-full bg-brand-500 transition-all" :style="{ width: (step * 25) + '%' }"></div>
            </div>

            <!-- Content scrolls inside the card so a text-heavy step (the welcome
                 list) is always fully reachable, whatever the window size. The
                 footer below stays pinned, so Next/Back are always visible. -->
            <div class="p-6 overflow-y-auto flex-1 min-h-0">
                <!-- step 1 -->
                <div v-if="step === 1" class="space-y-4">
                    <h1 class="text-2xl font-bold">Welcome to Defrag Racing Launcher</h1>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        A small companion app for
                        <a href="#" @click.prevent="openExternal('https://defrag.racing')" class="text-brand-400 hover:underline">defrag.racing</a>.
                        Here's what's inside:
                    </p>
                    <ul class="text-sm text-neutral-300 grid sm:grid-cols-2 gap-x-8 gap-y-1.5 pl-1">
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Auto-backup demos</strong> - every new <code class="text-xs bg-black/40 px-1 rounded">.dm_*</code> file in your demos folder gets uploaded to your account.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Instant demo player</strong> - hit play on any demo and watch it <strong>right inside the launcher</strong>, no separate game launch. Scrub, pause, change speed, seek with the keyboard.</span>
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
                            <span><strong>Records &amp; Maps</strong> - paginated leaderboards (VQ3 + CPM side-by-side) and the full map list with thumbnails, weapon/item/function icons and per-map info.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Run any map offline</strong> - one click on a map's <strong>VQ3</strong> / <strong>CPM</strong> button downloads it (if you don't have it) and launches straight into it.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Notifications</strong> - PB beats, world record takes, render-done events and account alerts, sorted by type.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>History</strong> - log of every <code class="text-xs bg-black/40 px-1 rounded">defrag://</code> server you joined, one-click Reconnect, plus the maps that played while you were on each server.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong><code class="text-xs bg-black/40 px-1 rounded">defrag://</code> links</strong> - click "Join" on the website, the launcher asks you to confirm, your engine launches.</span>
                        </li>
                        <li class="flex gap-2">
                            <span class="text-brand-400 mt-0.5">•</span>
                            <span><strong>Developer mode</strong> - optional: add custom engine launch arguments and your own named Quick-launch profiles (in Settings).</span>
                        </li>
                    </ul>
                    <p class="text-xs text-neutral-500 leading-relaxed pt-1">
                        Most of this needs a token from your defrag.racing account. The <code class="bg-black/40 px-1 rounded">defrag://</code> handler works without one. Setup takes under a minute.
                    </p>
                </div>

                <!-- step 3: token (optional - last because it's the only
                     skippable part of setup) -->
                <div v-else-if="step === 3" class="space-y-4">
                    <h2 class="text-xl font-bold">Account token <span class="text-sm font-normal text-neutral-500">(optional)</span></h2>
                    <p class="text-sm text-neutral-400 leading-relaxed">
                        A token links the launcher to your defrag.racing account. It unlocks:
                    </p>
                    <!-- Two columns on the wide card so the full feature list
                         stays short vertically - the token input below must be
                         reachable without scrolling. -->
                    <ul class="text-sm text-neutral-300 grid sm:grid-cols-2 gap-x-6 gap-y-1 pl-1 marker:text-brand-400">
                        <TokenFeatureList />
                    </ul>
                    <p class="text-xs text-neutral-500 leading-relaxed">
                        Auto-upload of new demos stays <strong>off by default</strong> - you turn it on later with the Start button on the dashboard.
                    </p>
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
                    <div v-if="tokenError" class="rounded border border-red-500/40 bg-red-500/10 p-3 text-xs text-red-200 space-y-1.5">
                        <div class="flex items-start gap-2">
                            <span class="text-red-400 mt-0.5 flex-shrink-0">✕</span>
                            <span>{{ tokenError }}</span>
                        </div>
                        <div class="text-red-300/80 pl-6">
                            Open the token page above and check you copied the token from the
                            <strong class="text-red-200">Launcher Tokens</strong> block under
                            <span class="font-mono">defrag.racing &gt; Settings &gt; Security</span> - not another token type.
                        </div>
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
                                Without a token these features will be <strong>disabled</strong> and visibly empty:
                            </p>
                            <ul class="text-xs text-amber-100 space-y-0.5 pl-1 rounded border border-amber-500/30 bg-amber-500/10 p-3">
                                <TokenFeatureList />
                            </ul>
                            <div class="text-xs text-emerald-300 font-semibold">These still work without a token:</div>
                            <ul class="text-xs text-emerald-200/90 space-y-0.5 pl-1 rounded border border-emerald-500/30 bg-emerald-500/10 p-3">
                                <TokenFreeFeatures />
                            </ul>
                            <p class="text-xs text-neutral-500">You can paste a token anytime later from Settings.</p>
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
                        <div class="flex items-center justify-between">
                            <div class="text-xs uppercase tracking-wider text-neutral-500">Engines</div>
                            <span v-if="engines.length" class="text-xs text-neutral-600">{{ engines.length }} found</span>
                        </div>
                        <div v-if="enginesLoading" class="text-sm text-neutral-500">Scanning…</div>
                        <div v-else-if="!engines.length" class="text-sm text-neutral-500">
                            None detected automatically.
                        </div>
                        <!-- Scroll the list (not the whole page) so the header and the
                             Selected / Demos / Next sections below stay on screen even
                             when many engines are detected. -->
                        <div v-if="engines.length" class="space-y-2 max-h-[40vh] overflow-y-auto pr-1 -mr-1">
                            <label
                                v-for="e in engines"
                                :key="e.path"
                                class="flex items-center gap-3 cursor-pointer bg-black/30 border border-white/10 rounded p-3 hover:bg-black/40"
                                :class="{ 'ring-1 ring-brand-500 bg-brand-500/5': selectedEngine === e.path }"
                            >
                                <input
                                    type="radio"
                                    name="engine"
                                    :value="e.path"
                                    :checked="selectedEngine === e.path"
                                    @change="selectEngine(e.path)"
                                    class="flex-shrink-0"
                                />
                                <div class="min-w-0 flex-1">
                                    <div class="flex items-center gap-2">
                                        <span class="font-semibold uppercase text-[10px] tracking-wider text-brand-400 flex-shrink-0">{{ e.kind }}</span>
                                        <span class="text-sm text-neutral-100 font-medium truncate">{{ pathFile(e.path) }}</span>
                                    </div>
                                    <div class="text-xs text-neutral-500 truncate" :title="prettyPath(e.path)">{{ pathDir(e.path) }}</div>
                                </div>
                            </label>
                        </div>

                        <button class="btn-ghost w-full" @click="pickManualEngine">Browse manually…</button>
                        <button class="text-xs text-neutral-500 hover:text-neutral-300" @click="rescanEngines">Rescan</button>
                    </div>

                    <div v-if="selectedEngine" class="space-y-2 pt-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Selected engine</div>
                        <div class="flex items-center gap-2 bg-emerald-500/5 border border-emerald-500/30 rounded p-3">
                            <svg class="w-4 h-4 text-emerald-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                            </svg>
                            <div class="flex-1 min-w-0">
                                <div class="text-sm text-neutral-100 font-medium truncate">{{ pathFile(selectedEngine) }}</div>
                                <div class="text-xs text-neutral-500 truncate" :title="prettyPath(selectedEngine)">{{ pathDir(selectedEngine) }}</div>
                            </div>
                            <button class="btn-ghost flex-shrink-0" @click="selectedEngine = null">Change</button>
                        </div>
                    </div>

                    <div v-if="selectedEngine || demosPath" class="space-y-2 pt-2">
                        <div class="text-xs uppercase tracking-wider text-neutral-500">Demos folder</div>
                        <div class="flex items-center gap-2 bg-black/30 border border-white/10 rounded p-3">
                            <div class="flex-1 text-sm text-neutral-200 break-all min-w-0">
                                {{ demosPath ? prettyPath(demosPath) : '(not detected - click Change)' }}
                            </div>
                            <button class="btn-ghost" @click="pickDemosFolder">Change</button>
                        </div>
                        <p v-if="demosError" class="text-xs text-red-300">{{ demosError }}</p>
                        <p class="text-xs text-neutral-500">Must be your engine's <code class="bg-black/40 px-1 rounded">defrag/demos</code> folder (or a subfolder of it).</p>
                    </div>

                    <p v-if="!canProceedFromEngine" class="text-xs text-amber-300/80 pt-1">
                        Pick your engine and demos folder to continue - both are required for the launcher (and even <code class="bg-black/40 px-1 rounded">defrag://</code> links) to work.
                    </p>
                </div>

                <!-- step 4: finish -->
                <div v-else class="space-y-4">
                    <h2 class="text-xl font-bold">All set</h2>
                    <ul class="text-sm text-neutral-300 space-y-1">
                        <li class="flex items-center gap-2">
                            <span :class="tokenSkipped ? 'text-amber-400' : 'text-brand-400'">{{ tokenSkipped ? '!' : '✓' }}</span>
                            <span v-if="tokenSkipped">Token skipped</span>
                            <span v-else-if="tokenSavedName">Signed in as <strong class="text-neutral-100">{{ tokenSavedName }}</strong></span>
                            <span v-else>Token stored</span>
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span class="truncate">Engine: {{ pathFile(selectedEngine) }}</span>
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="text-brand-400">✓</span>
                            <span class="break-all">Demos folder: {{ prettyPath(demosPath) }}</span>
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
                        <div class="font-semibold text-emerald-300 pt-1">These still work without a token:</div>
                        <ul class="space-y-0.5 pl-1 text-emerald-200/90">
                            <TokenFreeFeatures />
                        </ul>
                        <div class="pt-1">
                            You can add a token anytime from
                            <strong class="text-amber-200">Settings → Auto-upload token</strong>.
                        </div>
                        <button class="mt-1 text-amber-200 hover:underline font-semibold" @click="step = 3">
                            ← Go back and paste a token instead
                        </button>
                    </div>

                </div>
            </div>

            <!-- Sticky footer: navigation is always visible, independent of how
                 far the content above is scrolled. Back lets you revisit a step
                 before finishing; the right side is the step's primary action. -->
            <div class="flex-shrink-0 border-t border-white/10 p-4 flex items-center justify-between gap-2 bg-neutral-900">
                <button v-if="step > 1" class="btn-ghost" @click="goBack">← Back</button>
                <span v-else></span>

                <div class="flex items-center gap-2">
                    <template v-if="step === 1">
                        <button class="btn-primary" @click="step = 2">Next</button>
                    </template>
                    <template v-else-if="step === 2">
                        <button class="btn-primary" :disabled="!canProceedFromEngine" @click="step = 3">Next</button>
                    </template>
                    <template v-else-if="step === 3">
                        <button class="btn-ghost" @click="requestSkipToken">Skip - defrag:// only</button>
                        <button class="btn-primary" :disabled="!token.trim() || tokenSaving" @click="saveToken">
                            {{ tokenSaving ? 'Saving…' : 'Save & continue' }}
                        </button>
                    </template>
                    <template v-else>
                        <button class="btn-primary" :disabled="finishing" @click="finish">
                            {{ finishing ? 'Finishing…' : 'Open launcher' }}
                        </button>
                    </template>
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
