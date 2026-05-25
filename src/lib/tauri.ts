// Thin typed wrapper around Tauri's `invoke` so each call site doesn't
// have to remember the command name + payload shape. Keeping it in one
// file makes the Rust ↔ Vue contract easy to scan in one place.

import { invoke } from '@tauri-apps/api/core';

export interface LauncherConfig {
    engine_path: string | null;
    demos_path: string | null;
    auto_upload_enabled: boolean;
    include_subfolders: boolean;
    auto_update_enabled: boolean;
    /** Target CPU% for the hashing worker. 0 = no throttle. */
    cpu_throttle_pct: number;
    onboarding_completed: boolean;
    config_version: string | null;
}

export interface EngineCandidate {
    kind: 'odfe' | 'idfe' | 'other';
    path: string;
    display_name: string;
}

export type UploadStatus =
    | 'pending'
    | 'hashing'
    | 'uploading'
    | 'done'
    | 'duplicate'
    | 'error';

export interface PendingUpload {
    path: string;
    filename: string;
    status: UploadStatus;
    demo_id: number | null;
    error: string | null;
    /** "cache" = matched local cache (size+mtime); "server" = matched on
     *  defrag.racing by MD5. Null on non-duplicate statuses. */
    duplicate_reason: string | null;
    size_bytes: number | null;
    hash_throughput_bps: number | null;
    upload_throughput_bps: number | null;
}

export interface UploadStateSnapshot {
    items: PendingUpload[];
}

export const tauri = {
    getConfig: () => invoke<LauncherConfig>('get_config'),
    saveConfig: (cfg: LauncherConfig) => invoke<void>('save_config', { cfg }),
    completeOnboarding: () => invoke<void>('complete_onboarding'),
    previousVersion: () => invoke<string | null>('previous_version'),
    acknowledgeVersion: () => invoke<void>('acknowledge_version'),
    appVersion: () => invoke<string>('app_version'),

    saveToken: (token: string) => invoke<void>('save_token', { token }),
    hasToken: () => invoke<boolean>('has_token'),
    clearToken: () => invoke<void>('clear_token'),
    resetLauncher: () => invoke<void>('reset_launcher'),

    detectEngines: () => invoke<EngineCandidate[]>('detect_engines'),
    guessDemosPath: (enginePath: string) => invoke<string | null>('guess_demos_path', { enginePath }),

    startAutoUpload: () => invoke<void>('start_auto_upload'),
    stopAutoUpload: () => invoke<void>('stop_auto_upload'),
    pauseAutoUpload: () => invoke<void>('pause_auto_upload'),
    resumeAutoUpload: () => invoke<void>('resume_auto_upload'),
    isAutoUploadRunning: () => invoke<boolean>('is_auto_upload_running'),
    isAutoUploadPaused: () => invoke<boolean>('is_auto_upload_paused'),
    getUploadState: () => invoke<UploadStateSnapshot>('get_upload_state'),
    clearUploadCache: () => invoke<void>('clear_upload_cache'),
    getCpuThrottlePct: () => invoke<number>('get_cpu_throttle_pct'),
    /** Runtime override; does not persist to config. */
    setCpuThrottlePctRuntime: (pct: number) => invoke<void>('set_cpu_throttle_pct_runtime', { pct }),

    isAutostartEnabled: () => invoke<boolean>('is_autostart_enabled'),
    setAutostartEnabled: (enabled: boolean) => invoke<void>('set_autostart_enabled', { enabled }),

    handleProtocolUrl: (url: string) => invoke<string>('handle_protocol_url', { url }),
    launchEngine: () => invoke<void>('launch_engine'),
    getPendingDeepLink: () => invoke<string | null>('get_pending_deep_link'),
    confirmPendingDeepLink: () => invoke<string>('confirm_pending_deep_link'),
    cancelPendingDeepLink: () => invoke<void>('cancel_pending_deep_link'),
};
