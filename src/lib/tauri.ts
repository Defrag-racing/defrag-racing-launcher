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

    /** In-launcher join (Servers / History). Logs to history.json
     *  with the supplied enrichment + source so the History tab can
     *  show "joined ^1EU CPM I on bug22_slick" instead of just an
     *  IP. `source` defaults to "servers" on the backend. */
    handleProtocolUrl: (url: string, enrichment?: ConnectEnrichment | null, source?: string) =>
        invoke<string>('handle_protocol_url', {
            url,
            enrichment: enrichment ?? null,
            source: source ?? null,
        }),
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

    /** Paginated recent records. `physics` is "vq3" | "cpm", page is
     *  1-based. Returns the raw Laravel paginator (data + current_page
     *  + last_page + total + ...) so the view can render pager UI. */
    getRecords: (physics: 'vq3' | 'cpm', page: number) =>
        invoke<Paginated<RecordRow>>('get_records', { physics, page }),
    /** Paginated map list, newest first. `search` is an optional
     *  substring match on map name; empty/null means no filter. */
    getMaps: (page: number, search?: string | null) =>
        invoke<Paginated<MapRow>>('get_maps', { page, search: search || null }),

    /** One-shot "who am I" lookup. Launcher caches the result for the
     *  Profile button so the link works without a per-render fetch. */
    getMe: () => invoke<LauncherMe>('get_me'),

    /** Demo library: every .dm_* file in the configured demos folder
     *  with its known hash + demo_id from uploaded.json when
     *  available. Reads FS directly, no server call. */
    listDemos: () => invoke<DemoLibraryEntry[]>('list_demos'),

    /** Notifications feed for the bell badge + Notifications tab.
     *  Records (PB beats / WR takes) + system (render done, etc.)
     *  plus unread counts. */
    getNotifications: () => invoke<NotificationsFeed>('get_notifications'),

    /** Per-row toggle / bulk mark-read mutations. Each returns the
     *  fresh `unread` block so the bell badge stays in sync with what
     *  the server just changed - the frontend uses that to avoid an
     *  immediate getNotifications() reload after every click. */
    notificationRecordToggle: (id: number) =>
        invoke<NotificationActionResponse>('notification_record_toggle', { id }),
    notificationRecordsMarkRead: () =>
        invoke<NotificationActionResponse>('notification_records_mark_read'),
    notificationRecordsMarkUnread: () =>
        invoke<NotificationActionResponse>('notification_records_mark_unread'),
    notificationSystemToggle: (id: number) =>
        invoke<NotificationActionResponse>('notification_system_toggle', { id }),
    notificationSystemMarkRead: () =>
        invoke<NotificationActionResponse>('notification_system_mark_read'),
    notificationSystemMarkUnread: () =>
        invoke<NotificationActionResponse>('notification_system_mark_unread'),

    /** Queue a YouTube render for a local demo by hash. Server
     *  short-circuits if already queued. Response includes
     *  _http_status so the frontend can branch on 200 (queued) vs
     *  404 (needs_upload) vs 429 (quota) without a TS catch. */
    requestRender: (fileHash: string) =>
        invoke<RenderRequestResponse>('request_render', { fileHash }),

    /** Single-demo render status. Polls cheaper than getNotifications. */
    getRenderStatus: (fileHash: string) =>
        invoke<RenderStatusResponse>('get_render_status', { fileHash }),

    /** Re-queue a single demo that failed upload. Requires the watcher
     *  to be running - the Tauri side returns an error otherwise. */
    retryUpload: (path: string) => invoke<void>('retry_upload', { path }),

    /** Delete a demo from disk + clear its cache + queue rows. Used by
     *  the Library context menu. Surfaces filesystem errors to the
     *  caller; a missing file is silently accepted as "already gone". */
    deleteDemo: (path: string) => invoke<void>('delete_demo', { path }),

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

/** Laravel paginator wrapper. Same shape Laravel emits via
 *  Eloquent::paginate(); only the fields the launcher actually reads
 *  are typed. */
export interface Paginated<T> {
    data: T[];
    current_page: number;
    last_page: number;
    per_page: number;
    total: number;
}

/** One record row from /api/launcher/records. mdd_id links to a
 *  /profile/{mdd_id} page on the web; user is eager-loaded so the
 *  launcher can show a real player name (with country flag) when
 *  available. */
export interface RecordRow {
    id: number;
    name: string;
    country: string | null;
    mdd_id: number | null;
    mapname: string;
    rank: number | null;
    time: number;
    date_set: string;
    physics: string;
    mode: string;
    user?: {
        id: number;
        name: string;
        plain_name?: string | null;
        country?: string | null;
    } | null;
}

/** One map row from /api/launcher/maps. thumbnail is a storage-
 *  relative path (or full URL); resolve via storage/ prefix the
 *  same way the Servers view does for mapdata thumbnails. */
export interface MapRow {
    id: number;
    name: string;
    author: string | null;
    thumbnail: string | null;
    physics: string | null;
    gametype: string | null;
    is_nsfw: boolean;
    date_added: string | null;
}

/** Token owner identity for the Profile button. mdd_id is the public
 *  profile id on defrag.racing; null when the user hasn't linked their
 *  account to an mdd profile yet (the Profile button is disabled in
 *  that case). */
export interface LauncherMe {
    id: number;
    mdd_id: number | null;
    name: string;
    plain_name: string | null;
    country: string | null;
}

/** One demo file from the user's local folder. hash + demo_id are
 *  populated only for files that have been processed (have an entry
 *  in uploaded.json); never-seen files come back with both null and
 *  the Library view shows them as "not yet uploaded". */
export interface DemoLibraryEntry {
    path: string;
    filename: string;
    size_bytes: number;
    /** Unix-epoch seconds; format with locale on the frontend. */
    mtime: number;
    hash: string | null;
    demo_id: number | null;
    /** "done" | "duplicate" | null, mirrors cache.status. */
    upload_status: string | null;
}

/** One row from RecordNotification. Field names match the web's
 *  `protected $fillable` on the Eloquent model so the JSON forwarded
 *  by the launcher controller lands here unchanged. `mdd_id` links to
 *  the public profile page. `worldrecord=true` flips the card styling
 *  to gold ("someone took the WR" beats "someone beat your time"). */
export interface RecordNotificationRow {
    id: number;
    user_id: number;
    /** Player who took the record. Q3 color codes preserved. */
    name: string | null;
    country: string | null;
    physics: string | null;
    mode: string | null;
    time: number | null;
    /** Token owner's previous time on the map. Used to compute the
     *  delta the card shows in green. Null when the owner had no PB
     *  before (e.g. a fresh map). */
    my_time: number | null;
    mdd_id: number | null;
    record_player_id: number | null;
    mapname: string | null;
    date_set: string | null;
    worldrecord: boolean;
    read: boolean;
    created_at: string;
}

/** One row from system Notification (render done, alias suggestions,
 *  announcements, clan/tournament events, etc.). `type` drives icon
 *  + color + filter bucket on the Notifications tab. `before`,
 *  `headline`, `after` are the three text parts; clicking `url`
 *  opens the related page. Render notifications use `subheadline`
 *  for the demo download/play URL too. */
export interface SystemNotificationRow {
    id: number;
    user_id: number;
    type: string;
    before: string | null;
    headline: string | null;
    after: string | null;
    subheadline: string | null;
    image: string | null;
    url: string | null;
    read: boolean;
    created_at: string;
}

export interface NotificationsFeed {
    records: RecordNotificationRow[];
    system: SystemNotificationRow[];
    unread: { records: number; system: number; total: number };
}

/** Shape returned by the six mark-read mutations. Per-row toggle
 *  carries id + new read state; bulk variants omit both (every row
 *  changed). All variants include the fresh unread block so the bell
 *  badge can update without a separate /notifications fetch. */
export interface NotificationActionResponse {
    id?: number;
    read?: boolean;
    unread: { records: number; system: number; total: number };
}

/** Response from request_render. `_http_status` carries the original
 *  HTTP code so the view can branch:
 *   - 200 + success=true: queued, queue_position present
 *   - 200 + already_queued: showing existing status / youtube_url
 *   - 404 + needs_upload: launcher should upload first then retry
 *   - 429 + remaining=0: daily quota exhausted
 *   - 403: account is restricted */
export interface RenderRequestResponse {
    _http_status: number;
    success?: boolean;
    already_queued?: boolean;
    needs_upload?: boolean;
    id?: number;
    status?: string;
    queue_position?: number;
    remaining_today?: number;
    youtube_url?: string | null;
    youtube_video_id?: string | null;
    error?: string;
    remaining?: number;
}

/** Response from get_render_status. has_render=false means there's
 *  no RenderedVideo for this demo yet (or the demo isn't on the
 *  server at all - reason="demo_not_uploaded"). */
export interface RenderStatusResponse {
    has_render: boolean;
    reason?: string;
    demo_id?: number;
    id?: number;
    status?: string;
    youtube_url?: string | null;
    youtube_video_id?: string | null;
}
