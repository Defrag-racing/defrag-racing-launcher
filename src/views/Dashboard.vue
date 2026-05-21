<script setup lang="ts">
    import { onMounted, onUnmounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import type { Update } from '@tauri-apps/plugin-updater';
    import { tauri, type UploadStateSnapshot, type PendingUpload } from '../lib/tauri';
    import { checkForUpdate, runUpdate, type UpdateState } from '../lib/updater';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const config = useConfigStore();

    const queue = ref<UploadStateSnapshot>({ items: [] });
    const toggling = ref(false);
    const toggleError = ref<string | null>(null);

    // defrag:// deep-link toast. The backend emits `deep-link://result`
    // after every link arrives (from the browser, single-instance, or
    // manual trigger). We render a transient banner so the user sees
    // what happened before the engine grabs focus.
    type DeepLinkResult =
        | { ok: true; address: string }
        | { ok: false; error: string; url: string };
    const deepLink = ref<DeepLinkResult | null>(null);
    let deepLinkTimer: number | undefined;

    // Auto-update banner. Quiet on success (just shows "up to date" for
    // a beat); persistent on "Update available" until the user installs.
    //
    // We can't store the Update object inside `ref` — its private field
    // doesn't survive Vue's reactive proxy and breaks the TS type. Keep
    // the class instance in a plain `let` and have the ref carry only
    // serializable data needed for the template.
    const updateState = ref<UpdateState>({ kind: 'idle' });
    let pendingUpdate: Update | null = null;

    let unlisten: UnlistenFn | null = null;
    let unlistenDeepLink: UnlistenFn | null = null;
    let updateCheckTimer: number | undefined;

    // Re-check for updates every 6h while the launcher is alive. The
    // interval keeps firing even when the main window is hidden in the
    // tray — JS lifecycle is bound to the process, not the OS window —
    // so the "set and forget" user who installed once and stays in
    // tray for weeks still gets caught up on security patches.
    const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

    const checkUpdate = async () => {
        if (!config.config.auto_update_enabled) return;
        try {
            const update = await checkForUpdate();
            if (update) {
                pendingUpdate = update;
                updateState.value = { kind: 'available', version: update.version };
            }
        } catch {
            // Network blip; the next interval will retry.
        }
    };

    onMounted(async () => {
        queue.value = await tauri.getUploadState();
        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            queue.value = ev.payload;
        });
        unlistenDeepLink = await listen<DeepLinkResult>('deep-link://result', (ev) => {
            deepLink.value = ev.payload;
            if (ev.payload.ok) {
                window.clearTimeout(deepLinkTimer);
                deepLinkTimer = window.setTimeout(() => { deepLink.value = null; }, 4000);
            }
        });

        // Update check only fires if the user has it enabled. We treat
        // a network failure as "skip silently" rather than a red error
        // banner — most users opening the launcher won't care that the
        // updater couldn't reach defrag.racing right now.
        if (config.config.auto_update_enabled) {
            try {
                updateState.value = { kind: 'checking' };
                const update = await checkForUpdate();
                if (update) {
                    pendingUpdate = update;
                    updateState.value = { kind: 'available', version: update.version };
                } else {
                    updateState.value = { kind: 'idle' };
                }
            } catch {
                updateState.value = { kind: 'idle' };
            }

            // After the initial check, poll every 6h. Lightweight HTTP
            // GET against the manifest endpoint — no demo work, no
            // gameplay impact even if a check happens mid-frag.
            updateCheckTimer = window.setInterval(checkUpdate, UPDATE_CHECK_INTERVAL_MS);
        }
    });

    onUnmounted(() => {
        if (unlisten) unlisten();
        if (unlistenDeepLink) unlistenDeepLink();
        window.clearTimeout(deepLinkTimer);
        if (updateCheckTimer !== undefined) window.clearInterval(updateCheckTimer);
    });

    const dismissDeepLink = () => {
        deepLink.value = null;
        window.clearTimeout(deepLinkTimer);
    };

    const installUpdate = async () => {
        if (!pendingUpdate) return;
        await runUpdate(pendingUpdate, (s) => { updateState.value = s; });
    };

    const toggle = async () => {
        toggleError.value = null;
        toggling.value = true;
        try {
            if (config.autoUploadRunning) {
                await tauri.stopAutoUpload();
            } else {
                await tauri.startAutoUpload();
            }
            await config.refresh();
        } catch (e: any) {
            toggleError.value = e.toString();
        } finally {
            toggling.value = false;
        }
    };

    const statusLabel = (item: PendingUpload) => {
        switch (item.status) {
            case 'pending': return 'Waiting';
            case 'hashing': return 'Hashing';
            case 'uploading': return 'Uploading';
            case 'done': return 'Uploaded';
            case 'duplicate': return 'Already backed up';
            case 'error': return 'Error';
        }
    };

    const statusColor = (item: PendingUpload) => {
        switch (item.status) {
            case 'done': return 'text-emerald-400';
            case 'duplicate': return 'text-cyan-400';
            case 'error': return 'text-red-400';
            case 'uploading':
            case 'hashing': return 'text-brand-400';
            default: return 'text-neutral-500';
        }
    };
</script>

<template>
    <div class="flex-1 flex flex-col">
        <!-- top bar -->
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between">
            <div class="flex items-center gap-2">
                <div class="w-2 h-2 rounded-full" :class="config.autoUploadRunning ? 'bg-emerald-400' : 'bg-neutral-600'"></div>
                <div class="text-sm">
                    <span class="font-semibold">Auto-upload</span>
                    <span class="text-neutral-500 ml-1">{{ config.autoUploadRunning ? 'running' : 'off' }}</span>
                </div>
            </div>
            <div class="flex items-center gap-2">
                <button
                    class="px-3 py-1.5 rounded text-sm font-semibold"
                    :class="config.autoUploadRunning
                        ? 'bg-white/5 hover:bg-white/10 text-neutral-200'
                        : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-400'"
                    :disabled="toggling"
                    @click="toggle"
                >
                    {{ config.autoUploadRunning ? 'Stop' : 'Start' }}
                </button>
                <button
                    class="px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-sm text-neutral-300"
                    @click="router.push({ name: 'settings' })"
                >Settings</button>
            </div>
        </header>

        <p v-if="toggleError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ toggleError }}
        </p>

        <div
            v-if="deepLink"
            class="px-5 py-2 border-b text-xs flex items-center gap-2"
            :class="deepLink.ok
                ? 'bg-emerald-500/10 border-emerald-500/20 text-emerald-300'
                : 'bg-red-500/10 border-red-500/20 text-red-300'"
        >
            <span v-if="deepLink.ok">Connecting to <strong>{{ deepLink.address }}</strong>…</span>
            <span v-else>
                Couldn't open <code class="font-mono">{{ deepLink.url }}</code> — {{ deepLink.error }}
            </span>
            <button class="ml-auto text-neutral-400 hover:text-neutral-200" @click="dismissDeepLink">×</button>
        </div>

        <div
            v-if="updateState.kind === 'available'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300 flex items-center gap-3"
        >
            <span>Update <strong>v{{ updateState.version }}</strong> is available.</span>
            <button class="ml-auto px-2 py-0.5 rounded bg-brand-500/20 hover:bg-brand-500/30 font-semibold" @click="installUpdate">
                Install &amp; restart
            </button>
        </div>
        <div
            v-else-if="updateState.kind === 'downloading'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300"
        >
            Downloading update… {{ updateState.percent }}%
        </div>
        <div
            v-else-if="updateState.kind === 'installing'"
            class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300"
        >
            Installing… the launcher will restart in a moment.
        </div>
        <div
            v-else-if="updateState.kind === 'error'"
            class="px-5 py-2 border-b border-red-500/20 bg-red-500/10 text-xs text-red-300"
        >
            Update failed: {{ updateState.message }}
        </div>

        <!-- body -->
        <div class="flex-1 overflow-auto">
            <div v-if="!queue.items.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🎬</div>
                    <div class="text-neutral-300 font-semibold">No demos yet</div>
                    <p class="text-sm text-neutral-500">
                        <template v-if="config.autoUploadRunning">
                            The launcher is watching your demos folder. Record a run and it will appear here.
                        </template>
                        <template v-else>
                            Turn on auto-upload to start watching your demos folder. New demos will appear here as they are backed up.
                        </template>
                    </p>
                </div>
            </div>

            <ul v-else class="divide-y divide-white/[0.04]">
                <li v-for="item in queue.items" :key="item.path" class="px-5 py-3 flex items-center gap-3">
                    <div class="flex-1 min-w-0">
                        <div class="text-sm text-neutral-100 truncate">{{ item.filename }}</div>
                        <div class="text-xs text-neutral-500 truncate">{{ item.path }}</div>
                    </div>
                    <div class="text-xs font-semibold" :class="statusColor(item)">
                        {{ statusLabel(item) }}
                    </div>
                </li>
            </ul>
        </div>
    </div>
</template>
