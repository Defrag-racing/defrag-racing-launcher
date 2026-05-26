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
    /** Skip the defrag:// confirmation banner and launch the engine
     *  directly. Off by default to prevent accidental forum-link clicks
     *  from yeeting the user into a random server. */
    deep_link_auto_connect: boolean;
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
    /** Cumulative number of demos that have reached a terminal status
     *  (Done / Duplicate / Error) since the current watcher session
     *  started. NOT capped by the queue ceiling - use this for honest
     *  progress reporting on big rescans. Resets when the user
     *  Stops + Starts. */
    processed_count: number;
    /** Per-terminal-status session counters. Unbounded - render these
     *  in the summary strip instead of counting queue.items by status,
     *  which clamps at QUEUE_CAP and made rescans look stalled. */
    done_count: number;
    duplicate_count: number;
    error_count: number;
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

    /** Unix-epoch ms at which a current 429 backoff ends, or 0 if
     *  no rate-limit wait is active. Frontend polls every ~1s and
     *  renders a countdown banner while > Date.now(). */
    getRateLimitResumeAtMs: () => invoke<number>('get_rate_limit_resume_at_ms'),

    isAutostartEnabled: () => invoke<boolean>('is_autostart_enabled'),
    setAutostartEnabled: (enabled: boolean) => invoke<void>('set_autostart_enabled', { enabled }),

    handleProtocolUrl: (url: string) => invoke<string>('handle_protocol_url', { url }),
    launchEngine: () => invoke<void>('launch_engine'),
    getPendingDeepLink: () => invoke<string | null>('get_pending_deep_link'),
    /** Optional enrichment is logged into history.json so the History
     *  tab can show map/server name alongside the IP. Pass whatever
     *  the live server lookup found at click time. */
    confirmPendingDeepLink: (enrichment?: ConnectEnrichment) =>
        invoke<string>('confirm_pending_deep_link', { enrichment: enrichment ?? null }),
    cancelPendingDeepLink: () => invoke<void>('cancel_pending_deep_link'),

    getConnectionHistory: () => invoke<ConnectionEntry[]>('get_connection_history'),
    clearConnectionHistory: () => invoke<void>('clear_connection_history'),

    // Untyped on purpose: the JSON shape is owned by the Laravel
    // ServerListService and will grow new fields over time. Frontend
    // uses a minimal interface (DefragServer below) for the columns it
    // actually renders.
    getServers: () => invoke<{ servers: DefragServer[] }>('get_servers'),
};

/** Minimal shape for the columns the launcher renders. Mirrors the
 *  Laravel Server model + ServerListService enrichment. Add fields here
 *  as new UI features need them; the Tauri side passes the JSON through
 *  opaquely so the backend can grow without a launcher release. */
export interface DefragServer {
    id: number;
    /** Server name WITH Q3 color codes (^1, ^2, ^xFF, ...). Use
     *  plain_name for display unless you're rendering colored text. */
    name: string;
    plain_name?: string;
    ip: string;
    port: number;
    map: string;
    /** Physics string like "df.cpm.run", "mdf.vq3", etc. Used to
     *  decide vq3 vs cpm via substring match. */
    defrag: string;
    /** "run" / "team" / "ctf" / "freestyle" - server's primary
     *  gametype as classified by the scraper. */
    type?: string;
    /** Numeric defrag_gametype as a string (Laravel varchar). 5 = mixed
     *  (run + teamrun simultaneously, common on multi-mode servers). */
    defrag_gametype?: string;
    /** ISO country code for the server's host location (flag image). */
    location?: string | null;
    /** Currently-connected players. Snake-cased because Laravel's
     *  default toArray() snake-cases relation names. */
    online_players?: DefragPlayer[];
    mapdata?: { thumbnail?: string | null } | null;
    /** Per-user fields populated for the token owner; null when the
     *  user has no PB on this server's current map. */
    mytime_time?: number | null;
    mytime_date?: string | null;
    myrank_position?: number | null;
    myrank_total?: number | null;
    besttime_time?: number | null;
    besttime_name?: string | null;
    besttime_country?: string | null;
    besttime_date?: string | null;
}

export interface DefragPlayer {
    id?: number;
    name: string;
    plain_name?: string;
    country?: string | null;
    nospec?: boolean;
    spectators?: DefragPlayer[];
}

/** Optional enrichment passed to confirmPendingDeepLink so the History
 *  tab can show "joined ^1EU CPM I on bug22_slick" instead of just an
 *  IP. The frontend fills this from its live server lookup; auto-
 *  connect entries log without enrichment via the Rust path. */
export interface ConnectEnrichment {
    map?: string | null;
    server_name?: string | null;
    physics?: string | null;
}

/** One row from history.json. `source` is "auto" when the user opted
 *  into auto-connect Settings and the launcher launched the engine
 *  without a banner, "confirmed" when they pressed Connect on the
 *  pending banner. */
export interface ConnectionEntry {
    timestamp_ms: number;
    ip: string;
    port: number;
    map: string | null;
    server_name: string | null;
    physics: string | null;
    source: string;
}
