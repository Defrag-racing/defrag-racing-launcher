// Pinia store holding the full launcher state the UI needs at a glance:
// config (engine path, demos path, auto-upload flag), token presence, and
// whether the watcher is currently running. Every mutation goes through
// the Rust side, so the store is really just a cached projection.

import { defineStore } from 'pinia';
import { ref } from 'vue';
import { tauri, type LauncherConfig, type LauncherMe } from '../lib/tauri';

export const useConfigStore = defineStore('config', () => {
    const config = ref<LauncherConfig>({
        engine_path: null,
        demos_path: null,
        auto_upload_enabled: false,
        include_subfolders: false,
        auto_update_enabled: true,
        cpu_throttle_pct: 15,
        deep_link_auto_connect: false,
        onboarding_completed: false,
        config_version: null,
        developer_mode: false,
        custom_launch_args: '',
        launch_profiles: [],
    });
    const hasToken = ref(false);
    const autoUploadRunning = ref(false);
    const loaded = ref(false);
    /** Cached "who am I" so the Profile button works without a per-
     *  render fetch. Fetched once on refresh() when a token is
     *  present; null if no token / fetch failed / user has no mdd
     *  profile linked. */
    const me = ref<LauncherMe | null>(null);

    const refresh = async () => {
        config.value = await tauri.getConfig();
        hasToken.value = await tauri.hasToken();
        autoUploadRunning.value = await tauri.isAutoUploadRunning();
        if (hasToken.value) {
            try {
                me.value = await tauri.getMe();
            } catch {
                me.value = null;
            }
        } else {
            me.value = null;
        }
        loaded.value = true;
    };

    const save = async (patch: Partial<LauncherConfig>) => {
        config.value = { ...config.value, ...patch };
        await tauri.saveConfig(config.value);
    };

    return { config, hasToken, autoUploadRunning, loaded, me, refresh, save };
});
