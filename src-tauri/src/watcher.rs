//! Background demo watcher + upload worker.
//!
//! Design: one Tokio task owns the shared [`UploadState`]; the filesystem
//! watcher pushes `PendingUpload`s into it, and the worker drains them one
//! at a time (serial upload keeps memory bounded and is gentle on the
//! shared upload API). The frontend reads state through the
//! `get_upload_state` command and listens for `upload_state_changed` events
//! emitted whenever the vector mutates.
//!
//! Not persisted to disk - restart = empty queue. Failed uploads surface in
//! the UI and the user can hit "Retry all" to re-scan the demos folder; the
//! lookup-by-hash call catches demos that actually made it up before the
//! error, so retries are cheap.

use crate::api::{ApiError, Client};
use crate::cache::UploadCache;
use crate::hashing;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Notify};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpload {
    pub path: PathBuf,
    pub filename: String,
    pub status: UploadStatus,
    pub demo_id: Option<u64>,
    pub error: Option<String>,
    /// "cache" when we skipped the network because our local cache already
    /// said this file was uploaded (matched size+mtime); "server" when the
    /// server's lookup-by-hash confirmed it. Lets the UI explain "Already
    /// backed up - why?" rather than leaving the user guessing.
    pub duplicate_reason: Option<String>,
    pub size_bytes: Option<u64>,
    /// Bytes/sec on the hashing pass, populated once hashing finishes.
    pub hash_throughput_bps: Option<u64>,
    /// Bytes/sec for the upload itself (multipart payload ≈ file size).
    pub upload_throughput_bps: Option<u64>,
}

impl PendingUpload {
    fn new(path: PathBuf, filename: String) -> Self {
        Self {
            path,
            filename,
            status: UploadStatus::Pending,
            demo_id: None,
            error: None,
            duplicate_reason: None,
            size_bytes: None,
            hash_throughput_bps: None,
            upload_throughput_bps: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    Pending,
    Hashing,
    Uploading,
    Done,
    Duplicate,
    Error,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UploadStateSnapshot {
    pub items: Vec<PendingUpload>,
}

/// Cap on the number of rows kept in the visible queue. Anything older
/// than this gets dropped when a new row is inserted at the head. The
/// motivation is webview survival: each emit ships a copy of the full
/// snapshot and Vue runs a list-diff on it, so an unbounded queue plus
/// a rescan of a several-thousand-file folder can crash the webview
/// with the white-screen-of-death we saw in 0.1.6 testing. 500 is
/// generous enough for a normal session (the user can still scroll
/// through their recent activity) while keeping the IPC payload small.
const QUEUE_CAP: usize = 500;

/// Minimum gap between two `upload_state_changed` emits. During a tight
/// inner loop (rescan with pause-aborted hashing) the per-update emit
/// rate can exceed what Vue + the webview IPC can absorb; throttling
/// to ~20 emits/sec stays well under the danger zone while still
/// feeling live (a user looking at the queue doesn't notice 50ms).
const EMIT_MIN_GAP_MS: u64 = 50;

/// Floor for the post-hash idle period regardless of throttle math, so
/// even a hash that finished in 1ms still yields the runtime briefly
/// before the next file. Keeps the worker from monopolising the async
/// task scheduler on freakishly fast hardware.
const HASH_MIN_FLOOR_MS: u64 = 5;

/// Upper bound on a single post-hash idle period. With low throttle
/// settings (5%) and a slow hash (5s) the math would otherwise demand
/// a 95s wait - gives the user a hard responsiveness ceiling: if you
/// hit Pause / Speed-up, you wait at most this long before the next
/// file gets picked up.
const HASH_MAX_WAIT_MS: u64 = 30_000;

#[derive(Default)]
pub struct UploadState {
    inner: Mutex<Vec<PendingUpload>>,
    /// Pause gate for the worker. Set via `set_paused`; the worker checks
    /// this before pulling the next item and parks on `notify` while true.
    /// Held in an `Arc` so the hashing path can clone a handle into a
    /// `spawn_blocking` closure and abort mid-chunk on pause.
    paused: Arc<AtomicBool>,
    notify: Notify,
    /// Flipped to true by `update()` on every state mutation; cleared by
    /// the background emit-pump task right after it emits the snapshot
    /// to the webview. Decouples the hot mutation path from IPC + Vue
    /// reactivity cost, AND ensures the FINAL mutation of a burst still
    /// reaches the frontend (the previous in-update throttle would drop
    /// it because the next-update gap check would never run).
    dirty: AtomicBool,
    /// Target CPU duty-cycle for the hashing worker, in percent (0-100).
    /// 0 disables the throttle entirely (no idle wait between hashes).
    /// Live-mutable from commands so the Speed-up button on Dashboard
    /// takes effect immediately, without restarting the watcher.
    cpu_throttle_pct: AtomicU8,
    /// Unix-epoch ms at which a current 429 backoff ends, or 0 when no
    /// rate-limit wait is active. Set by the API client via the
    /// observer callback installed in start(); read by Tauri command
    /// `get_rate_limit_resume_at` so the Dashboard can render a
    /// countdown banner while uploads are paced down by the server.
    rate_limit_resume_at_ms: AtomicU64,
}

impl UploadState {
    pub fn snapshot(&self) -> UploadStateSnapshot {
        UploadStateSnapshot {
            items: self.inner.lock().unwrap().clone(),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        if !paused {
            // Wake any worker parked in `notified().await`.
            self.notify.notify_one();
        }
    }

    /// Hand out a cheap clone of the pause flag for blocking tasks that
    /// can't hold a `&UploadState` across an `.await`.
    fn pause_flag(&self) -> Arc<AtomicBool> {
        self.paused.clone()
    }

    pub fn cpu_throttle_pct(&self) -> u8 {
        self.cpu_throttle_pct.load(Ordering::Acquire)
    }

    pub fn set_cpu_throttle_pct(&self, pct: u8) {
        let clamped = pct.min(100);
        self.cpu_throttle_pct.store(clamped, Ordering::Release);
    }

    /// Unix-epoch ms at which the active 429 backoff ends, or 0 when
    /// not rate-limited. Read by the Tauri command.
    pub fn rate_limit_resume_at_ms(&self) -> u64 {
        self.rate_limit_resume_at_ms.load(Ordering::Acquire)
    }

    /// Set by the API client's rate-limit observer (see Client::set_rate_limit_observer).
    /// Passing 0 clears the countdown.
    pub fn set_rate_limit_resume_at_ms(&self, ms: u64) {
        self.rate_limit_resume_at_ms.store(ms, Ordering::Release);
        // Mark state dirty so the emit pump pushes a fresh snapshot
        // and the frontend can update its banner immediately rather
        // than waiting for an unrelated update to fire.
        self.dirty.store(true, Ordering::Release);
    }

    /// Idle period to wait AFTER a hash of `hash_duration`, computed to
    /// keep total CPU usage at the configured duty cycle. The math is
    /// straightforward: at target T%, the hash should occupy T% of any
    /// (hash + wait) cycle, so wait = hash * (100 - T) / T. Clamped to
    /// [HASH_MIN_FLOOR_MS, HASH_MAX_WAIT_MS] so a freakishly fast or
    /// slow hash doesn't translate to a degenerate sleep. A target of
    /// 0 (or out-of-range) disables the throttle entirely.
    async fn wait_after_hash(&self, hash_duration: Duration) {
        let target = self.cpu_throttle_pct.load(Ordering::Acquire);
        if target == 0 || target >= 100 {
            return;
        }
        let hash_ms = hash_duration.as_millis() as u64;
        let raw_wait_ms = hash_ms.saturating_mul(100 - target as u64) / (target as u64).max(1);
        let wait_ms = raw_wait_ms.clamp(HASH_MIN_FLOOR_MS, HASH_MAX_WAIT_MS);
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut Vec<PendingUpload>) -> R) -> R {
        f(&mut self.inner.lock().unwrap())
    }

    /// Find-or-insert by path, apply `f` to the entry, mark state
    /// dirty. The background emit-pump task (started in `start()`)
    /// notices the dirty flag on its next tick and emits the snapshot
    /// to the webview at most once per EMIT_MIN_GAP_MS - the previous
    /// in-update throttle dropped the LAST emit of a burst (no
    /// follow-up update to retry), so a cache-hit rescan of 300 demos
    /// in <50ms ended up showing zero rows in the UI. Pump model
    /// guarantees the final state always lands within one tick.
    ///
    /// Insertion goes at the head so the newest activity is visible
    /// without scrolling - matches user expectation of an "activity
    /// feed". Queue cap (QUEUE_CAP) is enforced here so a multi-
    /// thousand-file rescan can't blow up the snapshot payload.
    fn update(
        &self,
        _app: &AppHandle,
        path: &Path,
        filename: &str,
        f: impl FnOnce(&mut PendingUpload),
    ) {
        self.with_mut(|items| {
            if let Some(existing) = items.iter_mut().find(|i| i.path == path) {
                f(existing);
            } else {
                let mut new = PendingUpload::new(path.to_path_buf(), filename.to_string());
                f(&mut new);
                items.insert(0, new);
                if items.len() > QUEUE_CAP {
                    items.truncate(QUEUE_CAP);
                }
            }
        });
        self.dirty.store(true, Ordering::Release);
    }
}

/// Message sent to the worker task.
pub enum Message {
    FileAdded(PathBuf),
    /// Walk the folder once at startup. `recursive` mirrors the
    /// include_subfolders user setting so the rescan matches what the
    /// live watcher will pick up afterward.
    RescanFolder { folder: PathBuf, recursive: bool },
    /// Sent on resume so the worker re-processes anything that ended
    /// up back in `Pending` because pause aborted its hashing pass.
    /// Without this, paused-mid-hash files would stay Pending forever
    /// until a new filesystem event happened to nudge the queue.
    RedrivePending,
}

pub struct WatcherHandle {
    _debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
    worker: tauri::async_runtime::JoinHandle<()>,
    emit_pump: tauri::async_runtime::JoinHandle<()>,
    pub state: Arc<UploadState>,
    /// Kept so `resume_auto_upload` can poke the worker awake with
    /// `Message::RedrivePending` after lifting the pause gate.
    pub tx: tokio::sync::mpsc::UnboundedSender<Message>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        // Both background tasks hold clones of the AppHandle and the
        // shared UploadState - they need an explicit abort, otherwise
        // dropping the handle (Stop button, app close) leaves them
        // running forever and each Stop+Start leaks a new pair.
        self.worker.abort();
        self.emit_pump.abort();
    }
}

/// Start watching `demos_path` and return a handle whose drop stops the
/// watcher + worker. The token is captured up front - if the user rotates
/// it later, stop/start the watcher to pick up the new one.
pub fn start(
    app: AppHandle,
    demos_path: PathBuf,
    include_subfolders: bool,
    api_base_url: String,
    token: String,
    cpu_throttle_pct: u8,
) -> anyhow::Result<WatcherHandle> {
    crate::log_startup(&format!(
        "watcher::start: demos_path={:?} include_subfolders={} throttle={}%",
        demos_path, include_subfolders, cpu_throttle_pct
    ));
    let state = Arc::new(UploadState::default());
    state.set_cpu_throttle_pct(cpu_throttle_pct);
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    crate::log_startup("watcher::start: channel created");

    // Debounce bursts while Defrag is still writing the file. The debounce
    // window is deliberately generous (5s) to absorb Windows Defender post-
    // scan + any weird FS buffering after Quake's temp→demos rename, while
    // still feeling near-instant to the user. The rename itself is atomic,
    // so this is more about defensive coding than correctness.
    let tx_fs = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_secs(5),
        None,
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                for ev in events {
                    for path in ev.event.paths {
                        if is_demo_file(&path) && !is_in_temp_subfolder(&path) {
                            let _ = tx_fs.send(Message::FileAdded(path));
                        }
                    }
                }
            }
        },
    )?;
    let mode = if include_subfolders {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    crate::log_startup("watcher::start: debouncer built, calling watch()");
    debouncer.watcher().watch(&demos_path, mode)?;
    crate::log_startup("watcher::start: watch() ok");

    // Kick off a rescan on start so demos created while the launcher was
    // closed still get uploaded. The Message variant carries the recursive
    // flag so the worker uses the same setting as the watcher.
    let _ = tx.send(Message::RescanFolder {
        folder: demos_path.clone(),
        recursive: include_subfolders,
    });
    crate::log_startup("watcher::start: rescan message queued");

    let state_worker = state.clone();
    let app_worker = app.clone();
    // tauri::async_runtime::spawn instead of tokio::spawn - this is called
    // from a sync Tauri command, which on Windows has no Tokio runtime
    // entered, so a bare tokio::spawn panics with "there is no reactor
    // running". Tauri's wrapper drives an internal runtime that's always
    // available from command context.
    crate::log_startup("watcher::start: about to async_runtime::spawn worker_loop");
    let worker = tauri::async_runtime::spawn(worker_loop(rx, state_worker, app_worker, api_base_url, token));
    crate::log_startup("watcher::start: async_runtime::spawn returned");

    // Emit pump: ticks every EMIT_MIN_GAP_MS and forwards the current
    // snapshot to the webview if state has been mutated since the last
    // emit. Decouples the IPC cost from the hot mutation path and -
    // crucially - guarantees the LAST mutation of a burst still reaches
    // the frontend (the old in-update throttle dropped it because the
    // next-update-with-gap-check never came).
    let state_pump = state.clone();
    let app_pump = app.clone();
    let emit_pump = tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(EMIT_MIN_GAP_MS));
        // Skip the immediate-fire of the first tick - there's nothing
        // to emit before the worker has had a chance to mutate state.
        tick.tick().await;
        loop {
            tick.tick().await;
            if state_pump.dirty.swap(false, Ordering::AcqRel) {
                let _ = app_pump.emit("upload_state_changed", state_pump.snapshot());
            }
        }
    });

    Ok(WatcherHandle {
        _debouncer: debouncer,
        worker,
        emit_pump,
        state,
        tx,
    })
}

fn is_demo_file(p: &Path) -> bool {
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext
            .to_ascii_lowercase()
            .starts_with("dm_"),
        None => false,
    }
}

/// Quake writes the active recording to `<demos_folder>/temp/<file>` and
/// atomically renames into `<demos_folder>/<file>` on stop-record. So
/// anything under a `temp` subdirectory is an in-progress demo we must
/// never upload - it'd be truncated + the server would reject it.
fn is_in_temp_subfolder(p: &Path) -> bool {
    p.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| s.eq_ignore_ascii_case("temp"))
            .unwrap_or(false)
    })
}

async fn worker_loop(
    mut rx: mpsc::UnboundedReceiver<Message>,
    state: Arc<UploadState>,
    app: AppHandle,
    api_base_url: String,
    token: String,
) {
    let mut client = match Client::new(api_base_url, token) {
        Ok(c) => c,
        Err(e) => {
            log::error!("failed to build API client: {e}");
            return;
        }
    };

    // Wire up the 429 countdown: the API client pings this observer
    // whenever it starts/stops waiting on a Retry-After. Worker side
    // forwards into UploadState so a Tauri command can expose it and
    // the Dashboard can render a "resuming in Xs" banner.
    let state_for_observer = state.clone();
    client.set_rate_limit_observer(Arc::new(move |resume_at_ms| {
        state_for_observer.set_rate_limit_resume_at_ms(resume_at_ms.unwrap_or(0));
    }));

    // Single in-memory cache shared across all messages processed by this
    // worker. Reload from disk once on start so we pick up state from
    // previous launcher sessions.
    let mut cache = UploadCache::load();

    while let Some(msg) = rx.recv().await {
        // Park while paused. We register interest in notification FIRST
        // and only then check the flag, so a pause→resume that happens
        // between the check and the await isn't lost.
        loop {
            let notified = state.notify.notified();
            if !state.is_paused() {
                break;
            }
            notified.await;
        }

        match msg {
            Message::FileAdded(path) => {
                handle_file(&client, &state, &app, &mut cache, path).await;
            }
            Message::RedrivePending => {
                // Walk the queue and re-process anything still Pending.
                // handle_file is idempotent for Done/Duplicate (early
                // return) so being broad here is safe.
                let pending_paths: Vec<PathBuf> = state.with_mut(|items| {
                    items
                        .iter()
                        .filter(|u| u.status == UploadStatus::Pending)
                        .map(|u| u.path.clone())
                        .collect()
                });
                for p in pending_paths {
                    // Re-check pause between items: the user may
                    // pause again mid-redrive and we should respect
                    // that without finishing the whole queue.
                    loop {
                        let notified = state.notify.notified();
                        if !state.is_paused() {
                            break;
                        }
                        notified.await;
                    }
                    handle_file(&client, &state, &app, &mut cache, p).await;
                }
            }
            Message::RescanFolder { folder, recursive } => {
                // Two scan modes by user choice. Recursive uses walkdir
                // (already a dep) with follow_links off so a stray symlink
                // can't loop. is_in_temp_subfolder filters out Quake's
                // WIP-recording dir at any depth.
                //
                // The pause-park has to happen per-FILE here, not just at
                // the top of the outer match. Without it, a rescan that
                // started before pause was hit will tear through every
                // file in the folder back-to-back: each handle_file
                // notices the pause and aborts hashing within ms, but
                // the for-loop instantly moves to the next file and
                // does the same dance - so the user sees a flood of
                // Hashing→Pending status flips even though they hit
                // Pause. Parking between files actually halts the work.
                if recursive {
                    for entry in walkdir::WalkDir::new(&folder)
                        .follow_links(false)
                        .into_iter()
                        .filter_map(|e| e.ok())
                    {
                        let p = entry.path();
                        if entry.file_type().is_file()
                            && is_demo_file(p)
                            && !is_in_temp_subfolder(p)
                        {
                            loop {
                                let notified = state.notify.notified();
                                if !state.is_paused() {
                                    break;
                                }
                                notified.await;
                            }
                            handle_file(&client, &state, &app, &mut cache, p.to_path_buf()).await;
                        }
                    }
                } else if let Ok(entries) = std::fs::read_dir(&folder) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_file() && is_demo_file(&p) && !is_in_temp_subfolder(&p) {
                            loop {
                                let notified = state.notify.notified();
                                if !state.is_paused() {
                                    break;
                                }
                                notified.await;
                            }
                            handle_file(&client, &state, &app, &mut cache, p).await;
                        }
                    }
                }
            }
        }
    }
}

/// Compute bytes/sec from a (size, elapsed) pair. Guards against the
/// degenerate < 1ms case where dividing by a near-zero would explode.
fn throughput_bps(size: u64, elapsed: Duration) -> Option<u64> {
    let secs = elapsed.as_secs_f64();
    if secs < 0.001 {
        return None;
    }
    Some((size as f64 / secs) as u64)
}

async fn handle_file(
    client: &Client,
    state: &Arc<UploadState>,
    app: &AppHandle,
    cache: &mut UploadCache,
    path: PathBuf,
) {
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    // Skip files that already made it up in a previous session - identified
    // by presence in state with Done/Duplicate status.
    let already_present = state.with_mut(|items| {
        items
            .iter()
            .any(|i| i.path == path && matches!(i.status, UploadStatus::Done | UploadStatus::Duplicate))
    });
    if already_present {
        return;
    }

    let size_bytes = std::fs::metadata(&path).ok().map(|m| m.len());

    // Persistent cache: if the file's current size+mtime match what we
    // recorded last time we uploaded it, we don't need to re-hash or call
    // the server. Surface it as Done/Duplicate with reason="cache" so the
    // user can tell apart "we already uploaded this" from "the server
    // independently confirmed a hash match".
    if let Some(entry) = cache.get_if_fresh(&path) {
        let status = match entry.status.as_str() {
            "done" => UploadStatus::Done,
            _ => UploadStatus::Duplicate,
        };
        state.update(app, &path, &filename, |u| {
            u.status = status;
            u.demo_id = entry.demo_id;
            u.size_bytes = size_bytes;
            u.duplicate_reason = Some("cache".to_string());
            u.error = None;
        });
        return;
    }

    state.update(app, &path, &filename, |u| {
        u.status = UploadStatus::Hashing;
        u.size_bytes = size_bytes;
        u.error = None;
    });

    let t_hash = Instant::now();
    let pause_flag = state.pause_flag();
    let md5 = match tauri::async_runtime::spawn_blocking({
        let path = path.clone();
        move || hashing::md5_hex_cancellable(&path, &pause_flag)
    })
    .await
    {
        // Hash completed normally.
        Ok(Ok(Some(h))) => h,
        // Hash aborted because user hit Pause mid-stream. Drop status
        // back to Pending so the row UI shows it as waiting (rather
        // than stuck on Hashing forever), and bail. resume_auto_upload
        // sends a RedrivePending message that re-enters handle_file
        // for everything in Pending status.
        Ok(Ok(None)) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Pending;
                u.error = None;
                u.hash_throughput_bps = None;
            });
            return;
        }
        Ok(Err(e)) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Error;
                u.error = Some(format!("hash failed: {e}"));
            });
            return;
        }
        Err(e) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Error;
                u.error = Some(format!("hash task panicked: {e}"));
            });
            return;
        }
    };
    let hash_elapsed = t_hash.elapsed();
    let hash_bps = size_bytes.and_then(|s| throughput_bps(s, hash_elapsed));
    state.update(app, &path, &filename, |u| {
        u.hash_throughput_bps = hash_bps;
    });

    // Duty-cycle throttle: idle for a period proportional to how long the
    // hash took, sized so total CPU usage averages out to the configured
    // cpu_throttle_pct. Placed right after the hash so the sleep folds
    // into the API round-trip on the upload path (extra low CPU for big
    // files going up) but still enforces the target on the duplicate
    // path where lookup-by-hash returns immediately.
    state.wait_after_hash(hash_elapsed).await;

    // Pre-flight: is this already on the server?
    match client.lookup_by_hash(&md5).await {
        Ok(r) if r.exists => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Duplicate;
                u.demo_id = r.demo_id;
                u.duplicate_reason = Some("server".to_string());
            });
            cache.insert(&path, md5.clone(), "duplicate", r.demo_id);
            let _ = cache.save();
            return;
        }
        Ok(_) => {}
        Err(e) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Error;
                u.error = Some(format!("lookup failed: {e}"));
            });
            return;
        }
    }

    state.update(app, &path, &filename, |u| {
        u.status = UploadStatus::Uploading;
    });

    let t_up = Instant::now();
    let upload_result = client.upload_demo(&path, &md5).await;
    let up_bps = size_bytes.and_then(|s| throughput_bps(s, t_up.elapsed()));

    match upload_result {
        Ok(r) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Done;
                u.demo_id = Some(r.demo_id);
                u.upload_throughput_bps = up_bps;
            });
            cache.insert(&path, md5, "done", Some(r.demo_id));
            let _ = cache.save();
        }
        Err(ApiError::Duplicate { demo_id }) => {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Duplicate;
                u.demo_id = Some(demo_id);
                u.duplicate_reason = Some("server".to_string());
            });
            cache.insert(&path, md5, "duplicate", Some(demo_id));
            let _ = cache.save();
        }
        Err(e) => {
            // Don't cache errors - we want a retry on next start to either
            // succeed or surface the same error again.
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Error;
                u.error = Some(format!("{e}"));
            });
        }
    }
}
