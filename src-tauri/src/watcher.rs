//! Background demo watcher + upload worker.
//!
//! Design: one Tokio task owns the shared [`UploadState`]; the filesystem
//! watcher pushes `PendingUpload`s into it, and the worker drains them one
//! at a time (serial upload keeps memory bounded and is gentle on the
//! shared upload API). The frontend reads state through the
//! `get_upload_state` command and listens for `upload_state_changed` events
//! emitted whenever the vector mutates.
//!
//! Not persisted to disk — restart = empty queue. Failed uploads surface in
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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// backed up — why?" rather than leaving the user guessing.
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

/// Minimum gap between two hash STARTS, in milliseconds. Caps CPU usage
/// during rescans: at 100ms gap, a worst-case rescan-of-everything
/// pegs at ~10 hashes/sec, which on most CPUs stays well under 100%
/// single-core. Live FileAdded events also pass through this gate but
/// their natural rate (one demo per several seconds) is already below
/// the cap, so user-facing latency is unchanged.
///
/// Only the actual MD5 streaming path waits — cache hits and skips of
/// already-uploaded files bypass entirely. Otherwise a user with 5000
/// cached demos would sit through 8 minutes of no-op throttle waits on
/// every rescan.
const HASH_MIN_GAP_MS: u64 = 100;

#[derive(Default)]
pub struct UploadState {
    inner: Mutex<Vec<PendingUpload>>,
    /// Pause gate for the worker. Set via `set_paused`; the worker checks
    /// this before pulling the next item and parks on `notify` while true.
    /// Held in an `Arc` so the hashing path can clone a handle into a
    /// `spawn_blocking` closure and abort mid-chunk on pause.
    paused: Arc<AtomicBool>,
    notify: Notify,
    /// Last time we emitted an upload_state_changed event. Used to
    /// rate-limit emits — see `EMIT_MIN_GAP_MS`.
    last_emit_at: Mutex<Option<Instant>>,
    /// Start time of the most recent MD5 hashing pass. The next hash
    /// won't start until `HASH_MIN_GAP_MS` after this — that's the
    /// CPU throttle on rescan floods. Set in `wait_for_hash_slot`.
    last_hash_at: Mutex<Option<Instant>>,
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

    /// Block (asynchronously) until enough time has passed since the
    /// previous hash start to satisfy `HASH_MIN_GAP_MS`, then mark
    /// "now" as the most recent hash start. Caller invokes this
    /// immediately before kicking off the `spawn_blocking` MD5 pass.
    async fn wait_for_hash_slot(&self) {
        let wait_for = {
            let last = self.last_hash_at.lock().unwrap();
            last.map(|prev| {
                let target = prev + Duration::from_millis(HASH_MIN_GAP_MS);
                target.saturating_duration_since(Instant::now())
            })
        };
        if let Some(d) = wait_for {
            if !d.is_zero() {
                tokio::time::sleep(d).await;
            }
        }
        *self.last_hash_at.lock().unwrap() = Some(Instant::now());
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut Vec<PendingUpload>) -> R) -> R {
        f(&mut self.inner.lock().unwrap())
    }

    /// Find-or-insert by path, apply `f` to the entry, emit a change event.
    /// Insertion goes at the head so the newest activity is visible without
    /// scrolling — matches user expectation of an "activity feed". The
    /// emit is rate-limited (see EMIT_MIN_GAP_MS) and the queue is capped
    /// (QUEUE_CAP) so a burst of updates from a multi-thousand-file
    /// rescan can't crash the webview.
    fn update(
        &self,
        app: &AppHandle,
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

        // Throttle: only emit if EMIT_MIN_GAP_MS has elapsed since the
        // last one. We don't queue a trailing emit when we skip — when
        // the burst eventually stops, the *next* update will emit the
        // settled snapshot. Worst case the UI is up to 50ms stale
        // (imperceptible).
        let should_emit = {
            let mut last = self.last_emit_at.lock().unwrap();
            let now = Instant::now();
            let due = match *last {
                Some(prev) => now.duration_since(prev) >= Duration::from_millis(EMIT_MIN_GAP_MS),
                None => true,
            };
            if due {
                *last = Some(now);
            }
            due
        };
        if should_emit {
            let _ = app.emit("upload_state_changed", self.snapshot());
        }
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
    _worker: tauri::async_runtime::JoinHandle<()>,
    pub state: Arc<UploadState>,
    /// Kept so `resume_auto_upload` can poke the worker awake with
    /// `Message::RedrivePending` after lifting the pause gate.
    pub tx: tokio::sync::mpsc::UnboundedSender<Message>,
}

/// Start watching `demos_path` and return a handle whose drop stops the
/// watcher + worker. The token is captured up front — if the user rotates
/// it later, stop/start the watcher to pick up the new one.
pub fn start(
    app: AppHandle,
    demos_path: PathBuf,
    include_subfolders: bool,
    api_base_url: String,
    token: String,
) -> anyhow::Result<WatcherHandle> {
    crate::log_startup(&format!(
        "watcher::start: demos_path={:?} include_subfolders={}",
        demos_path, include_subfolders
    ));
    let state = Arc::new(UploadState::default());
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
    // tauri::async_runtime::spawn instead of tokio::spawn — this is called
    // from a sync Tauri command, which on Windows has no Tokio runtime
    // entered, so a bare tokio::spawn panics with "there is no reactor
    // running". Tauri's wrapper drives an internal runtime that's always
    // available from command context.
    crate::log_startup("watcher::start: about to async_runtime::spawn worker_loop");
    let worker = tauri::async_runtime::spawn(worker_loop(rx, state_worker, app_worker, api_base_url, token));
    crate::log_startup("watcher::start: async_runtime::spawn returned");

    Ok(WatcherHandle {
        _debouncer: debouncer,
        _worker: worker,
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
/// never upload — it'd be truncated + the server would reject it.
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
    let client = match Client::new(api_base_url, token) {
        Ok(c) => c,
        Err(e) => {
            log::error!("failed to build API client: {e}");
            return;
        }
    };

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
                // does the same dance — so the user sees a flood of
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

    // Skip files that already made it up in a previous session — identified
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

    // Show the file in the queue as Pending while we wait for our slot
    // in the hash rate-limit — honest about "we know about you, just
    // pacing the CPU". Without this, a rescan of many files would all
    // jump to Hashing at once even though only one is actually running.
    state.update(app, &path, &filename, |u| {
        u.status = UploadStatus::Pending;
        u.size_bytes = size_bytes;
        u.error = None;
    });

    state.wait_for_hash_slot().await;

    state.update(app, &path, &filename, |u| {
        u.status = UploadStatus::Hashing;
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
    let hash_bps = size_bytes.and_then(|s| throughput_bps(s, t_hash.elapsed()));
    state.update(app, &path, &filename, |u| {
        u.hash_throughput_bps = hash_bps;
    });

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
            // Don't cache errors — we want a retry on next start to either
            // succeed or surface the same error again.
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::Error;
                u.error = Some(format!("{e}"));
            });
        }
    }
}
