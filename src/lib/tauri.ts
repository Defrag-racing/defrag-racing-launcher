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
    isAutoUploadRunning: () => invoke<boolean>('is_auto_upload_running'),
    getUploadState: () => invoke<UploadStateSnapshot>('get_upload_state'),
};
