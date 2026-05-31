import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import type { Update } from '@tauri-apps/plugin-updater';
import { checkForUpdate, runUpdate, type CheckSource, type UpdateState } from '../lib/updater';
import { useConfigStore } from './config';

/** Shared auto-update state. Lives in a store so the Dashboard's
 *  available/downloading/installing banners and the Settings page's
 *  countdown + manual-check button stay in lock-step without each
 *  view running its own check loop. App.vue owns the boot check +
 *  recurring interval. */
export const useUpdaterStore = defineStore('updater', () => {
    // 15 min for testing; restore to 6h once we've confirmed the
    // auto-check flow + countdown render the way we want.
    const INTERVAL_MS = 15 * 60 * 1000;

    const state = ref<UpdateState>({ kind: 'idle' });
    const lastCheckAt = ref(0);
    const upToDateToast = ref(false);
    const manualBusy = ref(false);
    let pending: Update | null = null;
    let toastTimer: number | undefined;
    let intervalTimer: number | undefined;

    const intervalMs = computed(() => INTERVAL_MS);

    const runCheck = async (source: CheckSource) => {
        const config = useConfigStore();
        if (!config.config.auto_update_enabled) return;
        if (source === 'manual') {
            manualBusy.value = true;
            state.value = { kind: 'checking' };
        }
        try {
            const update = await checkForUpdate(source);
            if (update) {
                pending = update;
                state.value = { kind: 'available', version: update.version };
            } else if (source === 'manual') {
                state.value = { kind: 'idle' };
                upToDateToast.value = true;
                window.clearTimeout(toastTimer);
                toastTimer = window.setTimeout(() => { upToDateToast.value = false; }, 4000);
            } else {
                state.value = { kind: 'idle' };
            }
        } catch (e: any) {
            if (source === 'manual') {
                state.value = { kind: 'error', message: e?.toString?.() ?? 'Check failed' };
            }
        } finally {
            lastCheckAt.value = Date.now();
            if (source === 'manual') manualBusy.value = false;
        }
    };

    const install = async () => {
        if (!pending) return;
        await runUpdate(pending, (s) => { state.value = s; });
    };

    /** Start the boot check + 15 min interval. Called once from
     *  App.vue onMounted - subsequent calls are no-ops so a remount
     *  doesn't double the interval rate. */
    const start = async () => {
        if (intervalTimer !== undefined) return;
        await runCheck('boot');
        intervalTimer = window.setInterval(() => { void runCheck('auto'); }, INTERVAL_MS);
    };

    const stop = () => {
        if (intervalTimer !== undefined) {
            window.clearInterval(intervalTimer);
            intervalTimer = undefined;
        }
        window.clearTimeout(toastTimer);
    };

    return {
        state,
        lastCheckAt,
        upToDateToast,
        manualBusy,
        intervalMs,
        runCheck,
        install,
        start,
        stop,
    };
});
