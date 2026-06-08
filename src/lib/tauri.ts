// Thin typed wrapper around Tauri's `invoke` so each call site doesn't
// have to remember the command name + payload shape. Keeping it in one
// file makes the Rust ↔ Vue contract easy to scan in one place.

import { invoke } from '@tauri-apps/api/core';

/** A named engine launch configuration the user defined in developer mode. */
export interface LaunchProfile {
    id: string;
    name: string;
    args: string;
}

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
    /** Developer mode: reveals custom launch args + launch profiles in
     *  Settings. Off by default. */
    developer_mode: boolean;
    /** Extra args appended to the standard Quick launch (developer mode). */
    custom_launch_args: string;
    /** User-defined named launch profiles (developer mode). */
    launch_profiles: LaunchProfile[];
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

export interface TokenCheck {
    ok: boolean;
    /** 'ok' | 'invalid' | 'wrong_type' | 'network' | 'server' */
    kind: 'ok' | 'invalid' | 'wrong_type' | 'network' | 'server';
    message: string;
    name: string | null;
}

export interface HealthItem {
    id: string;
    title: string;
    status: 'ok' | 'warn' | 'error';
    detail: string;
    /** Repair action id to pass to healthRepair(), or null if not auto-fixable. */
    fix: string | null;
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
    /** Append a line to %APPDATA%\defrag\launcher\startup.log. Used by
     *  the updater + any other frontend code that wants its trace to
     *  survive across runs / show up next to Rust startup breadcrumbs. */
    logToFile: (msg: string) => invoke<void>('log_to_file', { msg }),

    saveToken: (token: string) => invoke<void>('save_token', { token }),
    /** Check a token with the server before saving. `kind` distinguishes
     *  an invalid token, a wrong-type token, and a network failure so the
     *  UI can show the right guidance. */
    validateToken: (token: string) => invoke<TokenCheck>('validate_token', { token }),
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
    /** Diagnose local launcher state (config, login, demos folder, cache/queue
     *  files, watcher, engine). The token row makes a network call. */
    healthCheck: () => invoke<HealthItem[]>('health_check'),
    /** Apply a repair action returned in a HealthItem.fix (e.g. reset_queue). */
    healthRepair: (action: string) => invoke<void>('health_repair', { action }),
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
    /** Launch the engine with extra arguments (developer-mode custom Quick
     *  launch + named profiles). `extraArgs` is the raw shell-style string;
     *  the backend splits it (quotes respected). */
    launchEngineArgs: (extraArgs: string) =>
        invoke<void>('launch_engine_args', { extraArgs }),
    /** Run a map offline: download its pk3 into baseq3 if missing (keyed by
     *  the original pk3 filename), then launch `+vq3`/`+cpm <map>`. */
    runMapOffline: (mapName: string, physics: 'vq3' | 'cpm', pk3: string | null) =>
        invoke<MapRunResult>('run_map_offline', { mapName, physics, pk3 }),
    /** List maps installed in the engine's baseq3 folder, paginated +
     *  name-filtered. The first call scans every pk3 once (cached to a
     *  manifest); later calls are cheap. Only a page is returned so the UI
     *  never loads the whole library at once. */
    listOfflineMaps: (page = 1, perPage = 24, search = '') =>
        invoke<OfflineMapPage>('list_offline_maps', { page, perPage, search }),
    /** Extract one offline map's levelshot from its pk3 as a data URL
     *  (cached). null when the pk3 has no levelshot for that map. */
    offlineMapThumb: (pk3Path: string, mapName: string) =>
        invoke<string | null>('offline_map_thumb', { pk3Path, mapName }),
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

    /** Lightweight bell-badge poll target. ~30B JSON; used by the
     *  App-level 180s poll instead of the full /notifications. */
    getNotificationsUnreadCount: () =>
        invoke<{ records: number; system: number; total: number }>('get_notifications_unread_count'),

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

    /** Bulk reconcile of completed renders: { hash: youtube_video_id }
     *  map (+ removed + synced_at cursor). since=0 = full sync, else a
     *  cheap delta. Replaces the per-row render-status warmup. */
    renderedIndex: (since: number) =>
        invoke<RenderedIndexResponse>('rendered_index', { since }),

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

    // ---- Embedded demo player (Windows only) ----------------------------
    /** Resolve the render width/height/aspect the engine would use for the
     *  embedded player, from the user's video cvars. The view sizes its
     *  render region to `aspect` so the HUD/FOV aren't distorted. Pass the
     *  screen size (for r_mode -2 = desktop). */
    engineDemoResolution: (desktopW: number, desktopH: number, fsGame?: string) =>
        invoke<RenderTarget>('engine_demo_resolution', {
            desktopW,
            desktopH,
            fsGame: fsGame ?? null,
        }),
    /** Start (or restart) embedded playback of the demo at absolute path
     *  `demo` (must live inside a `demos` folder - the backend derives
     *  fs_basepath/fs_game from it). `region` is the physical-pixel rect of the
     *  embed area; `aspect` from engineDemoResolution. Resolves with the control
     *  port. Errors on non-Windows. */
    demoPlayerStart: (
        demo: string,
        region: { x: number; y: number; w: number; h: number },
        aspect: number,
    ) =>
        invoke<number>('demo_player_start', {
            demo,
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            aspect,
        }),
    /** Start a side-by-side comparison: two engines, pane 0 (left) plays
     *  `demoA`, pane 1 (right) plays `demoB`, driven in lockstep. `region` is the
     *  full embed rect; the backend splits it into two halves. Token-gated in
     *  the UI. Resolves with the started pane count (2). Errors on non-Windows. */
    demoPlayerCompareStart: (
        demoA: string,
        demoB: string,
        region: { x: number; y: number; w: number; h: number },
        aspect: number,
    ) =>
        invoke<number>('demo_player_compare_start', {
            demoA,
            demoB,
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            aspect,
        }),
    /** Send a raw console line to EVERY pane: `timescale 0.5`, `demopause 1`,
     *  etc. In comparison mode it fans out to both engines. For seeking in
     *  comparison mode use demoPlayerSeekRelative so the panes stay aligned. */
    demoPlayerCommand: (line: string) => invoke<void>('demo_player_command', { line }),
    /** Seek every pane to the same playhead (ms from each demo's start), applying
     *  each pane's sync offset. Use this instead of `seekdemo` whenever a
     *  comparison is active so both engines stay locked together. */
    demoPlayerSeekRelative: (ms: number) =>
        invoke<void>('demo_player_seek_relative', { ms }),
    /** Set a pane's sync offset (ms) so two runs with different lead-ins line up.
     *  pane 0 = left, 1 = right. The next synchronized seek applies it. */
    demoPlayerSetOffset: (pane: number, ms: number) =>
        invoke<void>('demo_player_set_offset', { pane, ms }),
    /** Reposition the render region (window resize) + re-init the engine
     *  render window at the new size (vid_restart). */
    demoPlayerSetRegion: (
        region: { x: number; y: number; w: number; h: number },
        aspect: number,
    ) =>
        invoke<void>('demo_player_set_region', {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            aspect,
        }),
    /** Reposition the render window only (window MOVE) - no engine re-init.
     *  Cheap, call on every move event so the overlay follows the launcher. */
    demoPlayerReposition: (
        region: { x: number; y: number; w: number; h: number },
        aspect: number,
    ) =>
        invoke<void>('demo_player_reposition', {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            aspect,
        }),
    /** Stop playback: kill the engine, drop the render window. */
    demoPlayerStop: () => invoke<void>('demo_player_stop'),
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
/** One map observed on the server during a connected session. */
export interface MapPlay {
    map: string;
    timestamp_ms: number;
    physics: string | null;
}

export interface ConnectionEntry {
    /** Stable session id ("{ts}-{ip}-{port}"); empty for legacy entries. */
    id: string;
    timestamp_ms: number;
    ip: string;
    port: number;
    map: string | null;
    server_name: string | null;
    physics: string | null;
    source: string;
    /** Maps the server rotated through while the engine process was alive
     *  (per-session map history). Empty when nothing was recorded. */
    maps_played: MapPlay[];
}

/** Laravel paginator wrapper. The records endpoint now uses
 *  simplePaginate() so last_page / total are absent - presence of
 *  next_page_url decides whether "Next" is enabled. last_page + total
 *  remain optional so the older /api/launcher/maps endpoint (still
 *  paginate()) keeps typing. */
export interface Paginated<T> {
    data: T[];
    current_page: number;
    per_page: number;
    next_page_url?: string | null;
    prev_page_url?: string | null;
    last_page?: number;
    total?: number;
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
    /** Original pk3 filename/path. One pk3 can hold several maps, so the
     *  offline-run download keys off this, not the map name. */
    pk3: string | null;
    /** Comma-separated weapon / item / function codes (e.g. "rl,gl,lg"),
     *  rendered as icons over the thumbnail. See lib/mapIcons. */
    weapons: string | null;
    items: string | null;
    functions: string | null;
}

export interface MapRunResult {
    /** True if the pk3 was downloaded this run, false if already present. */
    downloaded: boolean;
}

/** A map installed locally in the engine's baseq3 folder (offline tab). */
export interface OfflineMap {
    name: string;
    pk3: string;
    pk3_path: string;
    has_levelshot: boolean;
}

/** One page of offline maps. Same shape as the online maps pager. */
export interface OfflineMapPage {
    data: OfflineMap[];
    total: number;
    current_page: number;
    last_page: number;
    per_page: number;
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

/** Response from rendered_index: compact reconcile of completed renders.
 *  `map` is { file_hash: youtube_video_id } (rebuild the watch URL from
 *  the id), `removed` lists hashes whose render is no longer
 *  completed/visible, `synced_at` is the cursor to pass as `since` next. */
export interface RenderedIndexResponse {
    map: Record<string, string>;
    removed: string[];
    synced_at: number;
}

// ---- Embedded demo player --------------------------------------------------

/** Render target the engine would use for the embedded player - the view
 *  sizes its render region to `aspect` (width / (height*pixelAspect)) so the
 *  defrag HUD/FOV match. Mirrors Rust `engine_video::RenderTarget`. */
export interface RenderTarget {
    width: number;
    height: number;
    aspect: number;
}

/** Payload of the `demo-player-status` event the engine emits ~10x/sec.
 *  Playhead = time - start; length = total - start (total is 0 until the
 *  view seeks once to measure it). */
export interface DemoPlayerStatus {
    /** Which engine this status is from: 0 = sole/left pane, 1 = right pane
     *  (only ever 1 during a side-by-side comparison). */
    pane: number;
    time: number;
    start: number;
    total: number;
    demo: boolean;
    paused: boolean;
    atend: boolean;
}
