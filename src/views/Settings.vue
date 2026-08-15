<script setup lang="ts">
    import { computed, onActivated, onMounted, onUnmounted, ref } from 'vue';
    import { useRoute, useRouter } from 'vue-router';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';
    import { openExternal } from '../lib/open';
    import { tauri, type CompsMode, type DemoAssocStatus, type DemoFolderEntry, type DemoFolderRoot, type EngineCandidate, type HealthItem, type LaunchProfile } from '../lib/tauri';
    import TokenFeatureList from '../components/TokenFeatureList.vue';
    import TokenFreeFeatures from '../components/TokenFreeFeatures.vue';
    import UpdateBanner from '../components/UpdateBanner.vue';
    import { useConfigStore } from '../stores/config';
    import { useUpdaterStore } from '../stores/updater';
    import { displayPath } from '../lib/path';
    import { LANGUAGES, locale, resolveLocale, setLocale, t } from '../lib/i18n';
    import { locale as osLocale } from '@tauri-apps/plugin-os';

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
        // Folders are read off disk, so re-entering Settings after making one
        // in Explorer shows it without a restart.
        void loadFolders();
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

    // ---- .dm_68 files -----------------------------------------------
    // The right-click entry is put in place by the installer and re-asserted
    // on every start. Becoming the DEFAULT program is only ever done from this
    // button, because most people already have DemoCleaner3 on the file type
    // and taking it silently would be a rude thing to do on their PC.
    const assoc = ref<DemoAssocStatus | null>(null);
    const assocBusy = ref(false);
    const assocNote = ref<string | null>(null);

    const refreshAssoc = async () => {
        try { assoc.value = await tauri.demoAssocStatus(); } catch { /* section stays hidden */ }
    };

    const makeDefault = async () => {
        assocBusy.value = true;
        assocNote.value = null;
        try {
            assoc.value = await tauri.demoAssocMakeDefault();

            // Windows keeps a signed UserChoice once somebody has picked a
            // program, and an application may not write it - forging it is
            // what gets installers flagged as malware. So when it does not
            // take, say what to do instead of leaving a dead button.
            if (!assoc.value.is_default) {
                assocNote.value = t('Windows keeps its own choice for this file type. Right-click any .dm_68 file, choose Open with, then Choose another app, pick Defrag Launcher and tick Always use this app.');
            }

            await config.save({ demo_assoc_asked: true });
        } catch (e: any) {
            assocNote.value = e?.toString?.() ?? t('Could not change the file association');
        } finally {
            assocBusy.value = false;
        }
    };

    onMounted(async () => {
        syncDevFromConfig();
        void refreshAssoc();
        engines.value = await tauri.detectEngines();
        appVersion.value = await tauri.appVersion();
        // Read the OS-level autostart state, not just our config -
        // catches the case where the user removed the registration
        // manually (Task Manager → Startup) outside the launcher.
        autostart.value = await tauri.isAutostartEnabled();
        void loadFolders();
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
            alert(t('Could not change the autostart setting: :reason', { reason: String(e) }));
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

    // ---- language ---------------------------------------------------
    // 'system' is a real choice, not a blank: it means "keep following this
    // PC", which is different from having picked English and different again
    // from never having looked. Stored as null so a machine that changes its
    // system language later follows it.
    const languageChoice = computed(() => config.config.language ?? 'system');

    const setLanguage = async (code: string) => {
        const saved = code === 'system' ? null : code;
        await config.save({ language: saved });
        setLocale(resolveLocale(saved, await osLocale().catch(() => null)));
    };

    const pickDemos = async () => {
        const picked = await openDialog({ multiple: false, directory: true });
        if (typeof picked === 'string') {
            await config.save({ demos_path: picked });
            void loadFolders();
        }
    };

    // ---- per-folder backup / visibility -----------------------------
    // The list is walked off disk on demand rather than kept in the config:
    // folders appear and disappear without telling us, and the config holds
    // only the answers that differ from the default.
    const folders = ref<DemoFolderRoot[]>([]);
    const foldersLoading = ref(false);
    const foldersError = ref<string | null>(null);
    /** Which folder is mid-write, so its two switches can't be double-clicked. */
    const folderBusy = ref<string | null>(null);

    const loadFolders = async () => {
        if (! config.config.demos_path) {
            folders.value = [];
            return;
        }
        foldersLoading.value = true;
        foldersError.value = null;
        try {
            folders.value = await tauri.listDemoFolders();
        } catch (e: any) {
            foldersError.value = e?.toString?.() ?? t('Could not read your demos folder');
            folders.value = [];
        } finally {
            foldersLoading.value = false;
        }
    };

    const setFolder = async (
        root: DemoFolderRoot,
        f: DemoFolderEntry,
        patch: { sync?: boolean; visible?: boolean },
    ) => {
        if (folderBusy.value) return;
        folderBusy.value = `${root.path}::${f.path}`;
        foldersError.value = null;
        try {
            // Every folder comes back: switching a parent off moves every
            // child that was inheriting from it, and those rows have to
            // redraw too.
            folders.value = await tauri.setDemoFolder(
                root.path,
                f.path,
                patch.sync ?? f.sync,
                patch.visible ?? f.visible,
            );
            await config.refresh();
        } catch (e: any) {
            foldersError.value = e?.toString?.() ?? t('Could not save that');
        } finally {
            folderBusy.value = null;
        }
    };

    /** Back a whole added folder up, or show it, or neither. */
    const setRoot = async (root: DemoFolderRoot, patch: { sync?: boolean; visible?: boolean }) => {
        if (folderBusy.value) return;
        folderBusy.value = root.path;
        foldersError.value = null;
        try {
            folders.value = await tauri.setDemoRoot(
                root.path,
                patch.sync ?? root.sync,
                patch.visible ?? root.visible,
            );
            await config.refresh();
        } catch (e: any) {
            foldersError.value = e?.toString?.() ?? t('Could not save that');
        } finally {
            folderBusy.value = null;
        }
    };

    /** Add a demos folder from anywhere - another drive, an archive. */
    const addRoot = async () => {
        const picked = await openDialog({ directory: true, multiple: false });
        if (typeof picked !== 'string') return;

        folderBusy.value = picked;
        foldersError.value = null;
        try {
            folders.value = await tauri.addDemoRoot(picked);
            await config.refresh();
        } catch (e: any) {
            foldersError.value = e?.toString?.() ?? t('Could not add that folder');
        } finally {
            folderBusy.value = null;
        }
    };

    /** Stop watching an added folder. The files are not touched. */
    const removeRoot = async (root: DemoFolderRoot) => {
        if (folderBusy.value) return;
        folderBusy.value = root.path;
        foldersError.value = null;
        try {
            folders.value = await tauri.removeDemoRoot(root.path);
            await config.refresh();
        } catch (e: any) {
            foldersError.value = e?.toString?.() ?? t('Could not save that');
        } finally {
            folderBusy.value = null;
        }
    };

    const toggleSubfolders = async (on: boolean) => {
        await config.save({ include_subfolders: on });
        await loadFolders();
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
        if (! confirm(t('Clear the stored token? Auto-upload will stop until you paste a new one.'))) return;
        await tauri.clearToken();
        try { await tauri.stopAutoUpload(); } catch {}
        await config.refresh();
    };

    const forceRecheck = async () => {
        if (reCheckBusy.value || reCheckCooldown.value > 0) return;
        if (! confirm(t('Re-check every demo against the server? This goes through the whole folder again and can take a while - watch the progress bar on the Demos tab.'))) return;
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
    // Settings used to be one column of fourteen cards. The groups below are
    // what somebody is actually looking for when they open this page, and the
    // last one is remembered so coming back lands where you left rather than
    // at the top.
    const SECTIONS = [
        { key: 'general',  label: 'General' },
        { key: 'game',     label: 'Game' },
        { key: 'demos',    label: 'Demos' },
        { key: 'comps',    label: 'Comps' },
        { key: 'notify',   label: 'Notifications' },
        { key: 'account',  label: 'Account' },
        { key: 'advanced', label: 'Advanced' },
    ] as const;

    const TAB_KEY = 'launcher.settings.tab.v1';
    const tab = ref<string>(
        SECTIONS.some(s => s.key === localStorage.getItem(TAB_KEY))
            ? (localStorage.getItem(TAB_KEY) as string)
            : 'general',
    );
    const setTab = (key: string) => {
        tab.value = key;
        localStorage.setItem(TAB_KEY, key);
    };

    const healthDot = (status: string) =>
        status === 'ok' ? 'bg-emerald-400' : status === 'warn' ? 'bg-amber-400' : 'bg-red-400';
</script>

<template>
    <div class="flex-1 flex flex-col">
        <header class="px-5 py-3 border-b border-white/10 flex items-center gap-3">
            <button class="text-sm text-neutral-400 hover:text-neutral-200" @click="router.back()">{{ $t('← Back') }}</button>
            <h1 class="font-semibold">{{ $t('Settings') }}</h1>
        </header>

        <div class="flex-1 flex min-h-0">
            <!-- One page of fourteen cards was a page nobody could find
                 anything on. The rail names what each group is about and
                 keeps the choice, so coming back lands where you left. -->
            <nav class="w-44 flex-shrink-0 border-r border-white/[0.06] p-3 space-y-1 overflow-auto">
                <button
                    v-for="s in SECTIONS"
                    :key="s.key"
                    class="w-full text-left px-3 py-2 rounded text-sm transition-colors"
                    :class="tab === s.key ? 'bg-brand-500/20 text-brand-200 font-semibold' : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/[0.04]'"
                    @click="setTab(s.key)"
                >{{ $t(s.label) }}</button>
            </nav>

            <div class="flex-1 overflow-auto p-5 space-y-4 max-w-2xl w-full">
            <div v-show="tab === 'general'" class="space-y-4">
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-start justify-between gap-3">
                        <div>
                            <div class="font-semibold">{{ $t('Language') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('The launcher follows your system language unless you pick one here.') }}
                            </div>
                        </div>
                        <select
                            class="bg-black/60 border border-white/10 rounded px-2 py-1.5 text-sm text-neutral-200 focus:border-brand-500/60 focus:outline-none"
                            :value="languageChoice"
                            @change="setLanguage(($event.target as HTMLSelectElement).value)"
                        >
                            <option value="system">{{ $t('Same as my system') }}</option>
                            <option v-for="l in LANGUAGES" :key="l.code" :value="l.code">{{ l.label }}</option>
                        </select>
                    </div>
                    <p v-if="locale !== 'en'" class="text-xs text-neutral-500">
                        {{ $t('Notifications sent by defrag.racing arrive already written and stay in English.') }}
                    </p>
                </section>

                <!-- Engine -->
                <!-- Autostart -->
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                    <div>
                        <div class="font-semibold">{{ $t('Start with the system') }}</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            {{ $t('Start quietly into the tray when you log in, so demo backup and server join links keep working without you having to open the launcher yourself.') }}
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

                <!-- Desktop notifications. The launcher spends its life minimised
                     behind a fullscreen game, so anything worth saying has to
                     leave the window or it arrives after it mattered. -->
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
                <!-- Auto-update status (read-only, informational) -->
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-center gap-2">
                        <span class="text-emerald-400">●</span>
                        <div class="font-semibold">{{ $t('Automatic updates: on') }}</div>
                    </div>
                    <div class="text-xs text-neutral-500 leading-relaxed">
                        {{ $t('The launcher checks defrag.racing and GitHub for a newer signed release on every start. This cannot be switched off, because security fixes have to reach everybody. When an update is available a banner appears on every tab, and right here with the full list of changes.') }}
                    </div>
                    <!-- Manual check + next-check countdown. Lives here
                         (not on the main dashboard) because it's a setting-
                         adjacent diagnostic, not something the user needs to
                         see every time the launcher opens. -->
                    <div class="flex items-center justify-between gap-3 pt-2 border-t border-white/[0.04]">
                        <div class="text-xs">
                            <span v-if="updater.state.kind === 'checking'" class="text-neutral-300">{{ $t('Checking…') }}</span>
                            <span v-else-if="updater.upToDateToast" class="text-emerald-400">✓ {{ $t('You are on the latest version') }}</span>
                            <span v-else-if="updater.state.kind === 'available'" class="text-brand-300">
                                {{ $t('Update :version is available.', { version: `v${updater.state.version}` }) }}
                            </span>
                            <span v-else-if="updater.state.kind === 'error'" class="text-red-300">
                                {{ $t('The last check failed:') }} {{ updater.state.message }}
                            </span>
                            <span v-else-if="countdownLabel" class="text-neutral-500">
                                {{ $t('Next check in') }} <span class="font-mono text-neutral-300">{{ countdownLabel }}</span>
                            </span>
                            <span v-else class="text-neutral-500">{{ $t('Idle') }}</span>
                        </div>
                        <button
                            class="btn-ghost text-xs disabled:opacity-50"
                            :disabled="updater.manualBusy"
                            @click="manualCheck"
                        >{{ updater.manualBusy ? $t('Checking…') : $t('Check now') }}</button>
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
            </div>

            <div v-show="tab === 'game'" class="space-y-4">
                <!-- Engine -->
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-start justify-between gap-3">
                        <div>
                            <div class="font-semibold">{{ $t('Defrag engine') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">{{ $t('Used when opening server join links.') }}</div>
                        </div>
                        <button class="btn-ghost" @click="pickEngine">{{ $t('Change') }}</button>
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
                            <div class="text-sm font-medium">{{ $t('Skip the join confirmation') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('Launch the engine immediately without asking. Useful if you join often and trust the links you click. An engine must be set.') }}
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
                <section v-if="assoc?.supported" class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div>
                        <div class="font-semibold">{{ $t('Demo files (.dm_68)') }}</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            {{ $t('Right-clicking a demo already offers to play it in the launcher. That entry sits next to whatever you already use and changes nothing else - your current program keeps the file type unless you say otherwise here.') }}
                        </div>
                    </div>

                    <div class="flex flex-wrap items-center gap-3 text-sm">
                        <span :class="assoc.context_menu ? 'text-emerald-300' : 'text-amber-300'">
                            {{ assoc.context_menu ? $t('Right-click entry registered') : $t('Right-click entry missing') }}
                        </span>
                        <span class="text-neutral-600">·</span>
                        <span :class="assoc.is_default ? 'text-emerald-300' : 'text-neutral-400'">
                            {{ assoc.is_default
                                ? $t('Double-clicking a demo opens the launcher')
                                : $t('Double-clicking a demo opens something else') }}
                        </span>
                        <button
                            v-if="!assoc.is_default"
                            class="ml-auto px-3 py-1.5 rounded text-sm font-semibold bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 disabled:opacity-50"
                            :disabled="assocBusy"
                            @click="makeDefault"
                        >{{ assocBusy ? $t('Working…') : $t('Open .dm_68 in the launcher') }}</button>
                    </div>

                    <p v-if="assocNote" class="text-xs text-amber-300">{{ assocNote }}</p>
                    <p v-else-if="assoc.default_owner && !assoc.is_default" class="text-xs text-neutral-600">
                        {{ $t('Currently owned by :owner.', { owner: assoc.default_owner }) }}
                    </p>
                </section>

                <!-- Token -->
            </div>

            <div v-show="tab === 'demos'" class="space-y-4">
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
                            <div class="font-semibold">{{ $t('Demos folder') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">{{ $t('The launcher watches this folder for new demos.') }}</div>
                            <div class="text-[11px] text-brand-400/80 mt-1">
                                {{ $t('Drives the Demos tab: auto-backup, the list of demos on disk, and YouTube renders.') }}
                            </div>
                        </div>
                        <button class="btn-ghost" @click="pickDemos">{{ $t('Change') }}</button>
                    </div>
                    <div class="text-sm text-neutral-300 break-all" :title="config.config.demos_path || ''">
                        {{ displayPath(config.config.demos_path) || '(not set)' }}
                    </div>
                    <div class="flex items-center justify-between gap-3 pt-2 border-t border-white/[0.05]">
                        <div>
                            <div class="text-sm font-medium">{{ $t('Include subfolders') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('Watch folders inside it too. Takes effect on the next Start.') }}
                            </div>
                        </div>
                        <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                            <input
                                type="checkbox"
                                class="sr-only peer"
                                :checked="config.config.include_subfolders"
                                @change="toggleSubfolders(($event.target as HTMLInputElement).checked)"
                            />
                            <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                            <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                        </label>
                    </div>

                    <!-- Every folder the launcher watches. The game's own is
                         first and cannot be removed; the rest are wherever the
                         person keeps demos - another drive, an archive, a
                         collection that predates the launcher. -->
                    <div class="pt-3 border-t border-white/[0.05]">
                        <div class="flex items-start justify-between gap-3">
                            <div>
                                <div class="text-sm font-medium">{{ $t('Your folders') }}</div>
                                <div class="text-xs text-neutral-500 mt-0.5">
                                    {{ $t('Demos can live on more than one drive. Add every folder you keep them in - each one decides on its own whether it is backed up to your account and whether it shows in the Demos list.') }}
                                </div>
                            </div>
                            <button class="btn-ghost flex-shrink-0" :disabled="folderBusy !== null" @click="addRoot">
                                {{ $t('Add a folder') }}
                            </button>
                        </div>

                        <p v-if="foldersError" class="mt-2 text-xs text-red-300">{{ foldersError }}</p>
                        <p v-else-if="foldersLoading && !folders.length" class="mt-3 text-xs text-neutral-500">
                            {{ $t('Reading your demos folder…') }}
                        </p>

                        <div class="mt-3 space-y-3">
                            <div
                                v-for="root in folders"
                                :key="root.path"
                                class="rounded-lg border border-white/[0.06] bg-black/20 p-3"
                            >
                                <div class="flex items-start gap-3">
                                    <div class="flex-1 min-w-0">
                                        <div class="text-sm text-neutral-200 break-all" :title="root.path">{{ root.path }}</div>
                                        <div class="text-[11px] text-neutral-500 mt-0.5">
                                            <span v-if="root.primary">{{ $t('Where your game records - this one is your demos folder above') }}</span>
                                            <span v-else-if="!root.exists" class="text-amber-300/80">{{ $t('Not there right now. A drive that is unplugged keeps its place in this list.') }}</span>
                                            <span v-else>
                                                {{ root.demos === 1 ? $t('1 demo') : $t(':count demos', { count: root.demos }) }}
                                                <span v-if="root.folders.length"> · {{ root.folders.length === 1 ? $t('1 subfolder') : $t(':count subfolders', { count: root.folders.length }) }}</span>
                                            </span>
                                        </div>
                                    </div>

                                    <!-- The game's own folder has no switches: turning
                                         backup off for it is what the button on the
                                         Demos tab does, and the same choice in two
                                         places is how one of them ends up lying. -->
                                    <template v-if="!root.primary">
                                        <label class="w-16 relative inline-flex items-center justify-center cursor-pointer" :title="root.sync ? $t('Backed up to defrag.racing') : $t('Not backed up')">
                                            <input
                                                type="checkbox"
                                                class="sr-only peer"
                                                :checked="root.sync"
                                                :disabled="folderBusy !== null"
                                                @change="setRoot(root, { sync: ($event.target as HTMLInputElement).checked })"
                                            />
                                            <span class="w-9 h-[18px] bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors block"></span>
                                            <span class="absolute left-0.5 top-0.5 w-3.5 h-3.5 bg-white rounded-full transition-transform peer-checked:translate-x-[18px]"></span>
                                        </label>
                                        <label class="w-16 relative inline-flex items-center justify-center cursor-pointer" :title="root.visible ? $t('Shown in the Demos list') : $t('Hidden from the Demos list')">
                                            <input
                                                type="checkbox"
                                                class="sr-only peer"
                                                :checked="root.visible"
                                                :disabled="folderBusy !== null"
                                                @change="setRoot(root, { visible: ($event.target as HTMLInputElement).checked })"
                                            />
                                            <span class="w-9 h-[18px] bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors block"></span>
                                            <span class="absolute left-0.5 top-0.5 w-3.5 h-3.5 bg-white rounded-full transition-transform peer-checked:translate-x-[18px]"></span>
                                        </label>
                                        <button
                                            class="text-neutral-500 hover:text-red-300 transition-colors flex-shrink-0"
                                            :title="$t('Stop watching this folder. Nothing on your disk is touched.')"
                                            :disabled="folderBusy !== null"
                                            @click="removeRoot(root)"
                                        >✕</button>
                                    </template>
                                </div>

                                <!-- Subfolders, only when they are watched at all.
                                     With that switch off there is nothing here to
                                     decide and an empty list would look broken. -->
                                <div v-if="config.config.include_subfolders && root.exists && root.folders.length" class="mt-3 pt-2 border-t border-white/[0.05] space-y-1">
                                    <div class="flex items-center gap-3 px-1 text-[11px] uppercase tracking-wide text-neutral-500">
                                        <span class="flex-1">{{ $t('Folder') }}</span>
                                        <span class="w-16 text-center">{{ $t('Back up') }}</span>
                                        <span class="w-16 text-center">{{ $t('Show') }}</span>
                                    </div>
                                    <div
                                        v-for="f in root.folders"
                                        :key="f.path"
                                        class="flex items-center gap-3 py-1.5 px-1 rounded hover:bg-white/[0.03]"
                                        :class="{ 'opacity-60': !f.visible && !f.sync }"
                                    >
                                        <div class="flex-1 min-w-0" :style="{ paddingLeft: `${(f.depth - 1) * 14}px` }">
                                            <div class="text-sm text-neutral-200 truncate" :title="f.path">
                                                <span class="text-neutral-600">{{ f.depth > 1 ? '└ ' : '' }}</span>{{ f.name }}
                                            </div>
                                            <div class="text-[11px] text-neutral-500">
                                                {{ f.demos === 1 ? $t('1 demo') : $t(':count demos', { count: f.demos }) }}
                                                <span v-if="f.inherited && (!f.sync || !f.visible)" class="text-neutral-600">
                                                    · {{ $t('following its parent') }}
                                                </span>
                                            </div>
                                        </div>
                                        <label class="w-16 relative inline-flex items-center justify-center cursor-pointer" :title="f.sync ? $t('Backed up to defrag.racing') : $t('Not backed up')">
                                            <input
                                                type="checkbox"
                                                class="sr-only peer"
                                                :checked="f.sync"
                                                :disabled="folderBusy !== null || !root.sync"
                                                @change="setFolder(root, f, { sync: ($event.target as HTMLInputElement).checked })"
                                            />
                                            <span class="w-9 h-[18px] bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors block"></span>
                                            <span class="absolute left-0.5 top-0.5 w-3.5 h-3.5 bg-white rounded-full transition-transform peer-checked:translate-x-[18px]"></span>
                                        </label>
                                        <label class="w-16 relative inline-flex items-center justify-center cursor-pointer" :title="f.visible ? $t('Shown in the Demos list') : $t('Hidden from the Demos list')">
                                            <input
                                                type="checkbox"
                                                class="sr-only peer"
                                                :checked="f.visible"
                                                :disabled="folderBusy !== null || !root.visible"
                                                @change="setFolder(root, f, { visible: ($event.target as HTMLInputElement).checked })"
                                            />
                                            <span class="w-9 h-[18px] bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors block"></span>
                                            <span class="absolute left-0.5 top-0.5 w-3.5 h-3.5 bg-white rounded-full transition-transform peer-checked:translate-x-[18px]"></span>
                                        </label>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>

                    <!-- CPU throttle -->
                    <div class="pt-3 border-t border-white/5">
                        <div class="mb-2">
                            <div class="text-sm font-medium">{{ $t('CPU usage while checking demos') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('How much of one CPU core the launcher may use while going through your demos. Lower is more comfortable while gaming and slower on a big folder. The Speed up button on the Demos tab overrides this while a backlog drains.') }}
                            </div>
                        </div>
                        <div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
                            <button
                                v-for="opt in [
                                    { label: $t('Background'), sub: '15%', value: 15 },
                                    { label: $t('Normal'),     sub: '25%', value: 25 },
                                    { label: $t('Fast'),       sub: '50%', value: 50 },
                                    { label: $t('No limit'),   sub: $t('max'), value: 0  },
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
                            {{ $t('Takes effect straight away, even while a backup is running.') }}
                        </p>
                    </div>
                </section>

                <!-- Comps guard. Lives next to the demos folder because it is a
                     rule about what happens to demos found in it. -->
            </div>

            <div v-show="tab === 'comps'" class="space-y-4">
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div>
                        <div class="font-semibold">{{ $t('Runs on comps maps') }}</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            {{ $t('Backed-up demos are public straight away. A run on a map being played in comps would therefore publish your time and your route in the middle of the round, and that cannot be taken back - so the launcher treats those demos separately. The map is read from the filename, and the site checks it again after reading the demo: if it was not a run of that map, the entry is withdrawn and it becomes an ordinary upload.') }}
                        </div>
                    </div>
                    <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
                        <button
                            v-for="opt in [
                                { value: 'ask',  label: $t('Ask me'),          sub: $t('hold it and let me choose') },
                                { value: 'auto', label: $t('Enter it'),        sub: $t('send it to comps for me') },
                                { value: 'off',  label: $t('Treat as normal'), sub: $t('back it up publicly') },
                            ]"
                            :key="opt.value"
                            class="px-3 py-2 rounded text-sm border transition-colors text-left"
                            :class="(config.config.comps_mode ?? 'ask') === opt.value
                                ? 'bg-brand-500/20 border-brand-500/60 text-brand-200'
                                : 'bg-white/5 border-white/10 hover:bg-white/10 text-neutral-300'"
                            @click="config.save({ comps_mode: opt.value as CompsMode })"
                        >
                            <div class="font-semibold">{{ opt.label }}</div>
                            <div class="text-xs text-neutral-500">{{ opt.sub }}</div>
                        </button>
                    </div>
                    <p v-if="(config.config.comps_mode ?? 'ask') === 'off'" class="text-xs text-amber-300">
                        {{ $t('With this off, a run you record on the map being played this week is published as soon as it is backed up, while the round is still running.') }}
                    </p>
                    <p v-else class="text-xs text-neutral-500">
                        {{ $t('Takes effect straight away, from the next demo onwards.') }}
                    </p>
                </section>

                <!-- Demo files. Windows only: the section hides itself where the
                     backend reports the whole idea unsupported. -->
            </div>

            <div v-show="tab === 'notify'" class="space-y-4">
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                        <div>
                            <div class="font-semibold">{{ $t('Desktop notifications') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('Your system notifications, so the launcher can reach you while you are in a game. Your PC asks for permission the first time one is sent.') }}
                            </div>
                        </div>
                        <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                            <input
                                type="checkbox"
                                class="sr-only peer"
                                :checked="config.config.notify_enabled"
                                @change="config.save({ notify_enabled: ($event.target as HTMLInputElement).checked })"
                            />
                            <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                            <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                        </label>
                    </div>

                    <div
                        v-if="config.config.notify_enabled"
                        class="pt-2 border-t border-white/[0.05] space-y-2"
                    >
                        <div
                            v-for="opt in [
                                {
                                    key: 'notify_comps' as const,
                                    title: $t('Comps'),
                                    detail: $t('A round opens, a demo of yours is being held for an answer, your run counts or does not, results are up.'),
                                },
                                {
                                    key: 'notify_records' as const,
                                    title: $t('Your records'),
                                    detail: $t('Somebody beats one of your times, or takes a world record.'),
                                },
                                {
                                    key: 'notify_system' as const,
                                    title: $t('Everything else from the site'),
                                    detail: $t('New maps, announcements, a finished render. The least urgent of the three, so it starts off.'),
                                },
                            ]"
                            :key="opt.key"
                            class="flex items-center justify-between gap-3"
                        >
                            <div class="min-w-0">
                                <div class="text-sm font-medium">{{ opt.title }}</div>
                                <div class="text-xs text-neutral-500 mt-0.5">{{ opt.detail }}</div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    :checked="config.config[opt.key]"
                                    @change="config.save({ [opt.key]: ($event.target as HTMLInputElement).checked })"
                                />
                                <div class="w-10 h-5 bg-neutral-700 peer-checked:bg-brand-500/60 rounded-full transition-colors"></div>
                                <div class="absolute left-0.5 top-0.5 w-4 h-4 bg-white rounded-full transition-transform peer-checked:translate-x-5"></div>
                            </label>
                        </div>
                    </div>
                </section>

                <!-- Auto-update is intentionally not user-toggleable. Security
                     fixes (token leak protection, signed-update bypasses, MSI
                     cleanup bugs that wipe user data, etc.) have to reach
                     every install without depending on the user remembering to
                     check Releases. The config field still exists and defaults
                     to true; the toggle UI is preserved below in case we ever
                     want to add an "expert mode" escape hatch.
            </div>

            <div v-show="tab === 'account'" class="space-y-4">
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
                            <div class="font-semibold">{{ $t('Account token') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                <a href="#" class="text-brand-400 hover:underline"
                                   @click.prevent="openExternal('https://defrag.racing/user/settings?tab=security')">
                                    {{ $t('Open the Launcher Tokens page on defrag.racing') }}
                                </a>.
                                {{ $t('The token is stored in your operating system keyring. It unlocks:') }}
                            </div>
                            <ul class="text-xs text-neutral-400 mt-1 space-y-0.5 pl-1">
                                <TokenFeatureList />
                            </ul>
                        </div>
                    </div>

                    <div v-if="config.hasToken" class="flex items-center gap-2">
                        <div class="flex-1 text-sm text-emerald-400 font-mono">• • • • • • • • • • •  {{ $t('(stored)') }}</div>
                        <button class="btn-ghost" @click="showTokenForm = !showTokenForm">{{ $t('Replace') }}</button>
                        <button class="btn-danger" @click="clearToken">{{ $t('Clear') }}</button>
                    </div>
                    <div v-else class="text-sm text-amber-300">
                        {{ $t('No token saved - the features above are disabled.') }}
                        <div class="text-emerald-300 font-semibold mt-2">{{ $t('Works without a token:') }}</div>
                        <ul class="text-xs text-emerald-200/90 mt-1 space-y-0.5 pl-1">
                            <TokenFreeFeatures />
                        </ul>
                    </div>

                    <div v-if="!config.hasToken || showTokenForm" class="flex gap-2">
                        <input
                            v-model="tokenInput"
                            type="text"
                            :placeholder="$t('Paste the token here')"
                            class="flex-1 bg-black/60 border border-white/10 rounded px-3 py-2 text-sm font-mono"
                        />
                        <button class="btn-primary" :disabled="!tokenInput.trim() || tokenSaving" @click="saveToken">
                            {{ tokenSaving ? $t('Saving…') : $t('Save') }}
                        </button>
                    </div>
                    <div v-if="tokenError" class="mt-2 rounded border border-red-500/40 bg-red-500/10 p-3 text-xs text-red-200 space-y-1.5">
                        <div class="flex items-start gap-2">
                            <span class="text-red-400 mt-0.5 flex-shrink-0">✕</span>
                            <span>{{ tokenError }}</span>
                        </div>
                        <div class="text-red-300/80 pl-6">
                            {{ $t('Create the token from the Launcher Tokens block, not another token type.') }}
                        </div>
                    </div>
                </section>

                <!-- Force re-check uploaded demos -->
            </div>

            <div v-show="tab === 'advanced'" class="space-y-4">
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                        <div>
                            <div class="font-semibold flex items-center gap-2">
                                <span>🛠️</span><span>{{ $t('Developer mode') }}</span>
                            </div>
                            <div class="text-xs text-neutral-500 mt-0.5 leading-relaxed">
                                {{ $t('Adds custom engine arguments and your own named quick-launch profiles. For people who tweak startup flags - leave it off if you are not sure.') }}
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
                            <div class="text-xs uppercase tracking-wider text-neutral-500">{{ $t('Custom launch arguments') }}</div>
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
                                {{ $t('Added to the Quick launch button. Quotes are respected, so a value with a space stays one argument.') }}
                            </div>
                        </div>

                        <!-- Named launch profiles. Each becomes its own button in
                             the top nav's launch menu. -->
                        <div class="space-y-2">
                            <div class="flex items-center justify-between">
                                <div class="text-xs uppercase tracking-wider text-neutral-500">{{ $t('Launch profiles') }}</div>
                                <button class="btn-ghost text-xs" @click="addProfile">+ {{ $t('Add a profile') }}</button>
                            </div>
                            <p v-if="profiles.length === 0" class="text-[11px] text-neutral-500">
                                {{ $t('No profiles yet. Add one to get an extra named launch button next to Quick launch.') }}
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
                                    :placeholder="$t('Name')"
                                    class="w-40 flex-shrink-0 bg-black/60 border border-white/10 rounded px-2 py-1.5 text-sm text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                                    @blur="persistProfiles"
                                />
                                <input
                                    v-model="p.args"
                                    type="text"
                                    spellcheck="false"
                                    :placeholder="$t('Arguments')"
                                    class="flex-1 min-w-0 bg-black/60 border border-white/10 rounded px-2 py-1.5 text-sm font-mono text-neutral-200 placeholder:text-neutral-600 focus:border-brand-500/60 focus:outline-none"
                                    @blur="persistProfiles"
                                    @keydown.enter="persistProfiles"
                                />
                                <button
                                    class="flex-shrink-0 px-2 py-1.5 rounded bg-red-500/10 hover:bg-red-500/20 text-red-300 text-xs"
                                    :title="$t('Remove this profile')"
                                    @click="removeProfile(p.id)"
                                >{{ $t('Remove') }}</button>
                            </div>
                            <p v-if="profiles.length > 0" class="text-[11px] text-neutral-500">
                                {{ $t('Each profile launches the engine with its own arguments and appears in the launch menu next to Quick launch. An engine has to be set above.') }}
                            </p>
                        </div>
                    </div>
                </section>

                <!-- Reset - wipes every setting and token and drops the user
                     back into the onboarding wizard. Lives in a red-tinted card
                     so it reads as destructive at a glance. (Re-run setup used
                     to be a separate button; it's gone - Reset is the canonical
                     way to redo setup, and every field is editable above anyway.) -->
                <!-- Force re-check uploaded demos -->
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 flex items-center justify-between gap-3">
                    <div>
                        <div class="font-semibold">{{ $t('Re-check uploaded demos') }}</div>
                        <div class="text-xs text-neutral-500 mt-0.5">
                            {{ $t('Forget what this PC remembers about which demos are already uploaded. The next Start asks the server about every demo again - useful if one was deleted on defrag.racing and you want to send it once more.') }}
                        </div>
                    </div>
                    <button class="btn-ghost flex-shrink-0" :disabled="reCheckBusy || reCheckCooldown > 0" @click="forceRecheck">
                        {{ reCheckBusy ? $t('Re-checking…') : (reCheckCooldown > 0 ? $t('Started - wait :seconds s', { seconds: reCheckCooldown }) : $t('Force a re-check')) }}
                    </button>
                </section>

                <!-- Check & repair -->
                <!-- Check & repair -->
                <section class="bg-neutral-900 border border-white/10 rounded-lg p-4 space-y-3">
                    <div class="flex items-center justify-between gap-3">
                        <div>
                            <div class="font-semibold">{{ $t('Check and repair') }}</div>
                            <div class="text-xs text-neutral-500 mt-0.5">
                                {{ $t('Go through what the launcher keeps on this PC - login, demos folder, backup records, activity list, the watcher - and fix anything broken. Your demos on the server are never touched.') }}
                            </div>
                        </div>
                        <button class="btn-ghost flex-shrink-0" :disabled="healthBusy" @click="runHealthCheck">
                            {{ healthBusy ? $t('Checking…') : (healthRan ? $t('Run it again') : $t('Run the check')) }}
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
                            >{{ healthFixing === item.id ? $t('Fixing…') : $t('Fix') }}</button>
                        </li>
                    </ul>
                </section>

                <!-- Autostart -->
                <section class="bg-red-500/5 border border-red-500/30 rounded-lg p-4 flex items-center justify-between">
                    <div>
                        <div class="font-semibold text-red-300">{{ $t('Reset the launcher') }}</div>
                        <div class="text-xs text-neutral-500 mt-0.5">{{ $t('Clear all settings and the stored token, then go through the setup again. Demos on your PC are not touched.') }}</div>
                    </div>
                    <button class="btn-danger" @click="showResetConfirm = true">{{ $t('Reset') }}</button>
                </section>
            </div>

            <div class="text-xs text-neutral-600 text-center pt-4">
                {{ $t('Defrag Racing Launcher') }} v{{ appVersion || '…' }}
            </div>
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
                <div class="font-semibold text-red-300 text-lg">{{ $t('Reset the launcher?') }}</div>
                <p class="text-sm text-neutral-300">
                    {{ $t('This clears everything the launcher stored: your account token, the engine path, the demos folder and all settings. You will be taken back through the setup.') }}
                </p>
                <p class="text-xs text-neutral-500">
                    {{ $t('Your demo files on this PC, and the demos already backed up to defrag.racing, are NOT touched.') }}
                </p>
                <div class="pt-1">
                    <label class="text-xs text-neutral-400">{{ $t('Type :word to confirm:', { word: 'yes' }) }}</label>
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
                    <button class="btn-ghost" @click="cancelReset">{{ $t('Cancel') }}</button>
                    <button
                        class="btn-danger disabled:opacity-40 disabled:cursor-not-allowed"
                        :disabled="!resetConfirmValid || resetting"
                        @click="resetLauncher"
                    >{{ resetting ? $t('Resetting…') : $t('Reset everything') }}</button>
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
