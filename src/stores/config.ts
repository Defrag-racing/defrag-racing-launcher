// Pinia store holding the full launcher state the UI needs at a glance:
// config (engine path, demos path, auto-upload flag), token presence, and
// whether the watcher is currently running. Every mutation goes through
// the Rust side, so the store is really just a cached projection.

import { defineStore } from 'pinia';
import { ref } from 'vue';
import { tauri, type LauncherConfig } from '../lib/tauri';

export const useConfigStore = defineStore('config', () => {
    const config = ref<LauncherConfig>({
        engine_path: null,
        demos_path: null,
        auto_upload_enabled: false,
        onboarding_completed: false,
    });
    const hasToken = ref(false);
    const autoUploadRunning = ref(false);
    const loaded = ref(false);

    const refresh = async () => {
        config.value = await tauri.getConfig();
        hasToken.value = await tauri.hasToken();
        autoUploadRunning.value = await tauri.isAutoUploadRunning();
        loaded.value = true;
    };

    const save = async (patch: Partial<LauncherConfig>) => {
        config.value = { ...config.value, ...patch };
        await tauri.saveConfig(config.value);
    };

    return { config, hasToken, autoUploadRunning, loaded, refresh, save };
});
