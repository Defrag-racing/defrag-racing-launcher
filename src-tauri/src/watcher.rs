//! Background demo watcher + upload worker.
//!
//! Design: one Tokio task owns the shared [`UploadState`]; the filesystem
//! watcher pushes `PendingUpload`s into it, and the worker drains them one
//! at a time (serial upload keeps memory bounded and is gentle on the
//! shared upload API). The frontend reads state through the
//! `get_upload_state` command and listens for `upload_state_changed` events
//! emitted whenever the vector mutates.
//!
//! Queue items are persisted to queue.json (next to uploaded.json) so the
//! activity feed survives an app restart. The hash cache (uploaded.json) is
//! still the source of truth for "is this on the server"; the queue file is
//! only the UI history, so any corruption / mismatch just costs an extra
//! lookup. Failed uploads surface in the UI and the user can hit "Retry
//! all" to re-scan the demos folder; the lookup-by-hash call catches demos
//! that actually made it up before the error, so retries are cheap.

use crate::api::{ApiError, Client};
use crate::cache::UploadCache;
use crate::comps::{CompsMode, CompsState};
use crate::hashing;
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    /// Set when the comps guard recognised this file as a run of a map the
    /// open round is playing. Carries the round it would be entered into, so
    /// the row can say WHICH map it matched and the user's answer needs no
    /// second guess. Persisted with the row, so a launcher restart doesn't
    /// silently forget that a demo is waiting on an answer.
    #[serde(default)]
    pub comps: Option<CompsHold>,
}

/// The comps round a held / entered demo belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompsHold {
    pub round_id: i64,
    pub comp_number: i64,
    /// Which physics' map the filename matched. The server reads the real
    /// physics out of the demo; this is only here so the row can name the map.
    pub physics: String,
    pub map: String,
    pub submission_id: Option<u64>,
}

impl From<crate::comps::CompsMatch> for CompsHold {
    fn from(m: crate::comps::CompsMatch) -> Self {
        Self {
            round_id: m.round_id,
            comp_number: m.comp_number,
            physics: m.physics,
            map: m.map,
            submission_id: None,
        }
    }
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
            comps: None,
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
    /// Recognised as a run of this week's comps map and deliberately NOT sent
    /// anywhere. Waiting for the user to say whether it is a comps entry or an
    /// ordinary demo. Not terminal - nothing has happened to the file yet -
    /// but handle_file returns early on it so a rescan never reopens the
    /// question every half hour.
    HeldForComps,
    /// Entered into the comps round. It is on the server but not public: comps
    /// demos stay hidden until the round ends.
    CompsEntered,
}

/// "We're done with this file" for processed-count purposes. Pending /
/// Hashing / Uploading mean work-in-flight and don't bump the counter;
/// any transition INTO Done / Duplicate / Error is one finished demo.
///
/// CompsEntered counts as finished too - the file went where it was going.
/// HeldForComps deliberately does not: it is a question waiting for an answer,
/// and counting it would report a demo as handled while it sits there.
fn is_terminal(s: UploadStatus) -> bool {
    matches!(
        s,
        UploadStatus::Done
            | UploadStatus::Duplicate
            | UploadStatus::Error
            | UploadStatus::CompsEntered
    )
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UploadStateSnapshot {
    pub items: Vec<PendingUpload>,
    /// Cumulative number of demos that have reached a terminal status
    /// (Done / Duplicate / Error) since the current watcher session
    /// started. NOT capped by QUEUE_CAP, so the UI can show real
    /// progress even when `items` is at the visual ceiling.
    /// Resets to 0 on every watcher::start.
    #[serde(default)]
    pub processed_count: u64,
    /// Per-terminal-status session counters. Unbounded - reflect every
    /// transition into the matching status this session, regardless of
    /// whether the row is still in the visible queue.
    #[serde(default)]
    pub done_count: u64,
    #[serde(default)]
    pub duplicate_count: u64,
    #[serde(default)]
    pub error_count: u64,
}

/// Cap on the number of rows kept in the visible queue. Anything older
/// than this gets dropped when a new row is inserted at the head.
/// Files that fall out of the queue are STILL fully processed - the
/// cache (uploaded.json) and the session counters below are
/// independent of this number, so QUEUE_CAP only affects what the
/// activity list shows, never what the worker actually does.
///
/// The activity feed is a "what happened recently" list, not the demo
/// library (that comes from list_demos, which carries each file's status
/// straight from the cache - so a row dropping out of this queue still
/// shows "Backed up" / "Already backed up" in the UI). The snapshot is
/// cloned and shipped over IPC on every emit (up to 20/sec) and the
/// frontend rebuilds its derived state from it each time, so the cost is
/// snapshot_size * emit_rate. 10000 let the whole library (~5400 rows)
/// land in every emit - a multi-MB payload 20x/sec that pegged the
/// webview at idle. 500 keeps the feed useful while bounding that cost;
/// the session counters (processed/done/duplicate/error) stay uncapped
/// so big rescans still report honest progress.
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

/// Lower bound between two writes of queue.json. The emit pump checks
/// dirty every EMIT_MIN_GAP_MS (50ms) and would otherwise burn through
/// hundreds of writes per second during a rescan. 1s keeps the file
/// fresh enough that an unclean kill loses at most a second of UI
/// history, while a Stop or graceful exit triggers a final save via
/// WatcherHandle::drop so nothing within the window is lost.
const QUEUE_SAVE_INTERVAL_SECS: u64 = 1;

/// How often we send a defensive RescanFolder message even though the
/// filesystem watcher should be picking up every new demo via OS-level
/// events. Two reasons we still need a periodic sweep:
///   1. The notify watcher can silently drop events on some platforms
///      (network drives, Defender exclusions, antivirus locking the
///      file briefly during the temp→demos rename). Without a periodic
///      catch-up the user has to remember to Stop+Start to recover.
///   2. Edge cases like the engine crashing mid-record and writing the
///      .dm_68 file in a non-standard way that doesn't fire a normal
///      "create/rename" event.
/// 30 minutes is the user-picked floor - the live notify watcher
/// catches new demos within 5s in the normal case, so the periodic
/// sweep is purely a safety net for the rare event the OS swallowed.
/// Scan cost is just directory enumeration + cache (size+mtime) check
/// per file; the disk impact is negligible even on HDDs.
const PERIODIC_RESCAN_SECS: u64 = 1800;

/// Cache status for a demo that went into a comps round. Kept distinct from
/// "done" because the two are not the same fact: a comps entry is on the
/// server but hidden until the round ends, and a rescan that reported it as a
/// finished backup would be telling the user their run is public when it is
/// not.
pub const CACHE_STATUS_COMPS: &str = "comps";

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
    /// Cumulative count of items that have transitioned into a terminal
    /// status (Done / Duplicate / Error) during the current watcher
    /// session. The queue itself is capped at QUEUE_CAP rows for
    /// webview survival, so this is the only honest progress number
    /// during a multi-thousand-demo rescan: the UI was showing "500
    /// total / 499 already backed up" forever and users assumed the
    /// worker had stalled when it was actually deep into row 1200+.
    processed_count: AtomicU64,
    /// Per-terminal-status session counters. Same role as
    /// `processed_count` but split so the UI can render an honest
    /// "X uploaded / Y already backed up / Z errors" instead of the
    /// queue-derived numbers, which max out at QUEUE_CAP and made big
    /// rescans look stalled at exactly the cap. Reset on
    /// watcher::start alongside processed_count.
    done_count: AtomicU64,
    duplicate_count: AtomicU64,
    error_count: AtomicU64,
    /// Normalised paths that reached Done/Duplicate THIS session. The
    /// periodic safety-net rescan (every PERIODIC_RESCAN_SECS) re-walks
    /// the whole folder; without this set it would re-confirm every
    /// already-backed-up demo from cache and re-tick the session
    /// counters, so a long-running session looks like it reprocessed the
    /// entire library ("10000 processed", "20 uploaded" for demos that
    /// never touched the network this run). The old guard checked the
    /// visible queue, but that's bounded by QUEUE_CAP - once a file falls
    /// out of it the guard went blind. This set is unbounded and survives
    /// queue eviction. Errors are deliberately NOT recorded so a rescan
    /// still retries them. Cleared on watcher::start.
    handled: Mutex<HashSet<PathBuf>>,
    /// True while a RescanFolder message is sitting in the worker's channel
    /// waiting to be picked up. Every enqueue site checks this first
    /// (`try_queue_rescan`), so a Force re-check spammed while a rescan is
    /// already queued - or racing the periodic rescan pump - can't stack
    /// multiple full-library re-hash passes behind each other (each pass
    /// re-hashes every demo; on a laptop several queued passes read as a
    /// frozen machine). Cleared by the worker the moment it dequeues the
    /// message, so a rescan requested DURING a running scan still queues
    /// one follow-up pass (the running scan may have missed the change).
    rescan_queued: AtomicBool,
}

impl UploadState {
    /// Rescan coalescing gate: returns true when the caller should actually
    /// send a `Message::RescanFolder` (none is queued yet), false when one is
    /// already waiting in the channel and sending another would only stack
    /// redundant full-folder re-hash passes.
    pub fn try_queue_rescan(&self) -> bool {
        !self.rescan_queued.swap(true, Ordering::AcqRel)
    }

    /// Counterpart of `try_queue_rescan`, called by the worker as soon as it
    /// dequeues a RescanFolder - from that point a new request may enqueue
    /// again (the now-running scan can miss changes that happen mid-walk).
    fn rescan_dequeued(&self) {
        self.rescan_queued.store(false, Ordering::Release);
    }

    pub fn snapshot(&self) -> UploadStateSnapshot {
        UploadStateSnapshot {
            items: self.inner.lock().unwrap().clone(),
            processed_count: self.processed_count.load(Ordering::Acquire),
            done_count: self.done_count.load(Ordering::Acquire),
            duplicate_count: self.duplicate_count.load(Ordering::Acquire),
            error_count: self.error_count.load(Ordering::Acquire),
        }
    }

    /// Zero all per-session counters. Called at the top of every
    /// watcher::start so the Dashboard summary reflects what the
    /// CURRENT run has done, not stale numbers from a previous
    /// Start+Stop cycle.
    pub fn reset_processed_count(&self) {
        self.processed_count.store(0, Ordering::Release);
        self.done_count.store(0, Ordering::Release);
        self.duplicate_count.store(0, Ordering::Release);
        self.error_count.store(0, Ordering::Release);
        self.handled.lock().unwrap().clear();
    }

    /// True if `path` already reached a successful terminal status
    /// (Done / Duplicate) earlier in this session - the periodic rescan
    /// uses this to skip re-confirming demos it already accounted for.
    fn is_handled(&self, path: &Path) -> bool {
        self.handled.lock().unwrap().contains(path)
    }

    /// Current queue status of a path, if it has a row. handle_file reads it
    /// to leave a demo held for comps exactly where it is: the periodic rescan
    /// walks the same folder every half hour and would otherwise re-ask a
    /// question the user has already been asked.
    fn status_of(&self, path: &Path) -> Option<UploadStatus> {
        self.inner.lock().unwrap().iter().find(|i| i.path == path).map(|i| i.status)
    }

    /// The comps round a held row belongs to, if any. Lets the user's answer
    /// use the round the demo was held for rather than whatever is open now -
    /// they are the same round in practice, but not while a week is turning
    /// over, and entering a demo into the wrong round is not recoverable by
    /// the user.
    fn comps_hold_of(&self, path: &Path) -> Option<CompsHold> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.path == path)
            .and_then(|i| i.comps.clone())
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

    /// Co-located with config.json / uploaded.json so a single config
    /// wipe clears everything. Errors here mean the platform config
    /// dir is unresolvable - exotic enough to surface up.
    fn queue_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
            .context("could not resolve platform config directory")?;
        let dir = dirs.config_dir().to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
        Ok(dir.join("queue.json"))
    }

    /// Pull the persisted queue from disk into this state, replacing
    /// any current contents. Best-effort: missing file or unparseable
    /// JSON yields an empty queue, the safe default. Called once at
    /// AppState::default() so the Dashboard sees its history on first
    /// mount, before Start has been pressed.
    ///
    /// Hashing / Uploading statuses lose meaning across an app restart
    /// (no worker is actually doing that work right now), so we
    /// normalise them down to Pending - the row stays visible with the
    /// right size/filename, and the next Start press will re-process it.
    pub fn load_persisted(&self) {
        let Ok(path) = Self::queue_path() else { return };
        let Ok(raw) = std::fs::read_to_string(&path) else { return };
        let Ok(snap) = serde_json::from_str::<UploadStateSnapshot>(&raw) else { return };
        let mut items = snap.items;

        // Reconcile every persisted row against the upload cache before it
        // ever reaches the UI. A row can be persisted non-terminal
        // (Pending/Hashing/Uploading) when the app closed or crashed in
        // the ~1s window between a successful upload (cache written
        // immediately) and the next throttled queue.json save - the cache
        // says "done", the queue still says "pending". Without this the
        // row shows "Backing up 0/1" forever AND the worker keeps re-
        // touching it on every rescan, melting CPU on an emit storm for a
        // file that is already on the server. cache.get() matches on the
        // NORMALISED path and ignores mtime drift, so it heals the row
        // even when the file was touched after upload (which is exactly
        // what makes the rescan's get_if_fresh freshness check miss it).
        let cache = UploadCache::load();
        let mut healed = 0usize;
        for item in &mut items {
            // Bring legacy verbatim-prefixed (`\\?\`) paths into the same
            // key space as the cache so the lookup below can hit.
            item.path = crate::cache::normalize(&item.path);
            if is_terminal(item.status) {
                continue;
            }
            match cache.get(&item.path).map(|e| (e.status.clone(), e.demo_id)) {
                Some((status, demo_id)) if status == "done" || status == "duplicate" => {
                    item.status = if status == "done" {
                        UploadStatus::Done
                    } else {
                        UploadStatus::Duplicate
                    };
                    item.demo_id = demo_id;
                    item.duplicate_reason = Some("cache".to_string());
                    item.error = None;
                    healed += 1;
                }
                _ => {
                    // Genuinely unfinished. Hashing/Uploading lose meaning
                    // across a restart (no worker is mid-flight right now),
                    // so drop them to Pending for a clean re-process on the
                    // next Start.
                    if matches!(item.status, UploadStatus::Hashing | UploadStatus::Uploading) {
                        item.status = UploadStatus::Pending;
                        item.error = None;
                    }
                }
            }
        }

        // A pre-fix queue.json (or an old build) may hold the entire
        // library; apply the same ceiling the live insert path enforces so
        // we don't reload tens of thousands of rows into every snapshot.
        if items.len() > QUEUE_CAP {
            items.truncate(QUEUE_CAP);
        }

        let still_pending = items.iter().filter(|i| !is_terminal(i.status)).count();
        if healed > 0 || still_pending > 0 {
            crate::log_startup(&format!(
                "load_persisted: {} rows; healed {} stale-pending from cache; {} still pending",
                items.len(),
                healed,
                still_pending
            ));
        }

        *self.inner.lock().unwrap() = items;
    }

    /// Snapshot the current queue and atomically write it to disk via
    /// .tmp + rename so a crash mid-save can't corrupt the file. Called
    /// by the emit pump (rate-limited to ~1s) and on WatcherHandle drop
    /// so the Stop button doesn't lose the last few hundred ms of state.
    pub fn save_persisted(&self) -> Result<()> {
        let path = Self::queue_path()?;
        let snap = self.snapshot();
        let raw = serde_json::to_string(&snap)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }

    /// Wipe the persisted queue from disk. Called by `reset_launcher`
    /// so a full reset really does start with a blank Dashboard.
    pub fn clear_persisted() -> Result<()> {
        let path = Self::queue_path()?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {:?}", path))?;
        }
        Ok(())
    }

    /// Drop every row from the in-memory queue and emit an update.
    /// Used by reset_launcher so the Dashboard clears instantly without
    /// waiting for a restart to re-read the (now empty) queue.json.
    pub fn clear_items(&self) {
        self.with_mut(|items| items.clear());
        self.dirty.store(true, Ordering::Release);
    }

    /// Drop a single row by path. Called by the Library context menu's
    /// Delete action so the deleted file disappears from the activity
    /// feed too, not just the cache. No-op when the row isn't there
    /// (it may never have been queued, e.g. file never made it past
    /// the watcher).
    pub fn remove_path(&self, path: &Path) {
        self.with_mut(|items| items.retain(|i| i.path != path));
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
        // Returns Some((status, fresh_upload, was_terminal)) whenever the
        // item is terminal after applying `f`, regardless of whether it
        // just transitioned or was already terminal. The COUNTING decision
        // is made below against the session `handled` set, not on the
        // transition - because load_persisted pre-loads queue rows as
        // terminal (to heal stuck "Backing up 0/1"), and the old
        // "non-terminal -> terminal" check silently skipped those, leaving
        // "processed this session" stuck a few hundred below the real
        // library size forever (4942/5409 and never moving).
        let terminal = self.with_mut(|items| {
            if let Some(existing) = items.iter_mut().find(|i| i.path == path) {
                let was_terminal = is_terminal(existing.status);
                f(existing);
                if is_terminal(existing.status) {
                    Some((existing.status, existing.duplicate_reason.is_none(), was_terminal))
                } else {
                    None
                }
            } else {
                let mut new = PendingUpload::new(path.to_path_buf(), filename.to_string());
                f(&mut new);
                let final_status = new.status;
                let fresh_upload = new.duplicate_reason.is_none();
                items.insert(0, new);
                if items.len() > QUEUE_CAP {
                    items.truncate(QUEUE_CAP);
                }
                if is_terminal(final_status) {
                    Some((final_status, fresh_upload, false))
                } else {
                    None
                }
            }
        });
        if let Some((status, fresh_upload, was_terminal)) = terminal {
            match status {
                UploadStatus::Error => {
                    // Count an error only on the first failure this touch,
                    // not on every rescan re-confirmation. Errors are NOT
                    // added to `handled`, so a later rescan / retry can
                    // still pick them up.
                    if !was_terminal {
                        self.processed_count.fetch_add(1, Ordering::AcqRel);
                        self.error_count.fetch_add(1, Ordering::AcqRel);
                    }
                }
                _ => {
                    // Done / Duplicate: count once per file per session,
                    // keyed by the `handled` set (HashSet::insert returns
                    // true only the first time). This also catches the
                    // load_persisted pre-loaded rows, so the count reaches
                    // the real library size; and it doubles as the
                    // periodic-rescan skip set so we never re-count.
                    if self.handled.lock().unwrap().insert(path.to_path_buf()) {
                        self.processed_count.fetch_add(1, Ordering::AcqRel);
                        // Only a real, this-session upload counts as
                        // "uploaded"; a Done re-confirmed from cache (it
                        // carries a duplicate_reason) is "already backed up".
                        if matches!(status, UploadStatus::Done) && fresh_upload {
                            self.done_count.fetch_add(1, Ordering::AcqRel);
                        } else {
                            self.duplicate_count.fetch_add(1, Ordering::AcqRel);
                        }
                    }
                }
            }
        }
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
    /// Force re-check: blank every cached upload status (keeping hashes)
    /// in the worker's in-memory cache and persist it, so the RescanFolder
    /// that follows actually re-verifies with the server instead of hitting
    /// stale in-memory entries. Must be sent BEFORE the RescanFolder.
    ResetCacheStatuses,
    /// The user answered a held demo: enter it into the comps round.
    CompsEnter(PathBuf),
    /// The user answered a held demo: upload it the ordinary way. The guard
    /// is skipped for this one file only - the answer is about this demo, not
    /// about the setting.
    CompsUploadNormally(PathBuf),
}

/// Who decided what happens to a file, on this pass through handle_file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompsDecision {
    /// Nobody yet - consult the guard and the configured mode.
    Guard,
    /// The user said "enter it", so the guard's opinion no longer matters.
    Enter,
    /// The user said "upload it normally", so the guard is skipped.
    Normal,
}

pub struct WatcherHandle {
    _debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
    worker: tauri::async_runtime::JoinHandle<()>,
    emit_pump: tauri::async_runtime::JoinHandle<()>,
    rescan_pump: tauri::async_runtime::JoinHandle<()>,
    pub state: Arc<UploadState>,
    /// Kept so `resume_auto_upload` can poke the worker awake with
    /// `Message::RedrivePending` after lifting the pause gate, and
    /// `clear_upload_cache` can request an immediate rescan.
    pub tx: tokio::sync::mpsc::UnboundedSender<Message>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        // All three background tasks hold clones of the AppHandle and
        // the shared UploadState - they need explicit aborts, otherwise
        // dropping the handle (Stop button, app close) leaves them
        // running forever and each Stop+Start leaks them.
        self.worker.abort();
        self.emit_pump.abort();
        self.rescan_pump.abort();
        // Final save so the Stop button doesn't lose the last <1s of
        // state mutations (emit_pump's save is rate-limited and the
        // very last burst may have been dropped). The Arc lives on
        // in AppState so this only persists - it does not free state.
        let _ = self.state.save_persisted();
    }
}

/// Start watching `demos_path` and return a handle whose drop stops the
/// watcher + worker. The token is captured up front - if the user rotates
/// it later, stop/start the watcher to pick up the new one.
///
/// `state` is shared with `AppState`, so the upload queue (and any items
/// loaded from queue.json at app boot) survives a Stop+Start cycle. The
/// handle keeps its own clone for the worker / emit pump / Drop save.
pub fn start(
    app: AppHandle,
    state: Arc<UploadState>,
    comps: Arc<CompsState>,
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
    // The Arc is shared with AppState so its non-queue runtime flags
    // would otherwise carry over from the previous watcher: a Pause +
    // Stop + Start cycle would re-enter with paused=true and look
    // frozen. Reset the transient ones at the top of every start.
    state.set_paused(false);
    state.set_rate_limit_resume_at_ms(0);
    state.set_cpu_throttle_pct(cpu_throttle_pct);
    // The session counter is meant to track "what did THIS Start do",
    // so zero it on every fresh start. Items in the queue (loaded
    // from disk) stay where they are.
    state.reset_processed_count();
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    crate::log_startup("watcher::start: channel created");

    // Debounce window absorbs the temp/ → demos/ atomic rename + any
    // Defender post-scan latching, then fires Message::FileAdded.
    // Atomic rename is instant in practice, so this is defensive
    // coding rather than a correctness requirement - generous values
    // are safe. 30s trades "demo appears in queue ~instantly" for a
    // calmer UX where the row doesn't pop up the moment you stop
    // recording. Periodic rescan (PERIODIC_RESCAN_SECS) is the
    // upper bound on "where is my demo?" anyway.
    let tx_fs = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_secs(30),
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
    if state.try_queue_rescan() {
        let _ = tx.send(Message::RescanFolder {
            folder: demos_path.clone(),
            recursive: include_subfolders,
        });
    }
    // Then redrive anything restored from queue.json still in Pending. The
    // rescan walks disk paths; a persisted Pending row keyed by a slightly
    // different (e.g. normalised) path can be missed by it and otherwise
    // hang forever ("Backing up 0/1" with nothing happening) until a manual
    // Stop+Start. RedrivePending re-processes by the queue rows' own paths,
    // and is idempotent (Done/Duplicate rows early-return in handle_file).
    let _ = tx.send(Message::RedrivePending);
    crate::log_startup("watcher::start: rescan + redrive messages queued");

    let state_worker = state.clone();
    let app_worker = app.clone();
    // tauri::async_runtime::spawn instead of tokio::spawn - this is called
    // from a sync Tauri command, which on Windows has no Tokio runtime
    // entered, so a bare tokio::spawn panics with "there is no reactor
    // running". Tauri's wrapper drives an internal runtime that's always
    // available from command context.
    crate::log_startup("watcher::start: about to async_runtime::spawn worker_loop");
    let worker = tauri::async_runtime::spawn(worker_loop(
        rx,
        state_worker,
        comps,
        app_worker,
        api_base_url,
        token,
    ));
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
        // Save-to-disk runs off the same dirty flag as the emit but
        // capped to once per QUEUE_SAVE_INTERVAL so a busy rescan doesn't
        // turn into a write storm. The Drop impl does a final flush, so
        // anything inside the throttle window still lands on disk before
        // a Stop or app exit.
        let mut last_saved = Instant::now() - Duration::from_secs(QUEUE_SAVE_INTERVAL_SECS);
        loop {
            tick.tick().await;
            if state_pump.dirty.swap(false, Ordering::AcqRel) {
                let _ = app_pump.emit("upload_state_changed", state_pump.snapshot());
                if last_saved.elapsed() >= Duration::from_secs(QUEUE_SAVE_INTERVAL_SECS) {
                    let _ = state_pump.save_persisted();
                    last_saved = Instant::now();
                }
            }
        }
    });

    // Periodic rescan as a safety net. The notify-based watcher
    // catches new demos via OS events but can silently miss some on
    // edge platforms (network drives, antivirus locking, weird
    // engine crashes). Firing RescanFolder every PERIODIC_RESCAN_SECS
    // means "I recorded a run and it didn't appear" recovers on its
    // own within one tick instead of needing Stop+Start. handle_file
    // is idempotent for already-processed files (cache hit OR queue
    // hit early-return), so a redundant rescan is cheap.
    let tx_rescan = tx.clone();
    let demos_for_rescan = demos_path.clone();
    let state_rescan = state.clone();
    let rescan_pump = tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(PERIODIC_RESCAN_SECS));
        // Skip the immediate-fire of the first tick - watcher::start
        // already queues the initial rescan, no point doing it twice.
        tick.tick().await;
        loop {
            tick.tick().await;
            // Coalesce with any rescan already waiting (e.g. a user Force
            // re-check) - stacking another full pass helps nobody.
            if !state_rescan.try_queue_rescan() {
                continue;
            }
            if tx_rescan
                .send(Message::RescanFolder {
                    folder: demos_for_rescan.clone(),
                    recursive: include_subfolders,
                })
                .is_err()
            {
                // Worker side hung up; nothing more we can do.
                break;
            }
        }
    });

    Ok(WatcherHandle {
        _debouncer: debouncer,
        worker,
        emit_pump,
        rescan_pump,
        state,
        tx,
    })
}

/// Run a single file through the same pipeline the worker uses, from outside
/// the worker.
///
/// Only for the case where there is no worker: a demo held for comps stays in
/// the queue after Stop, and the two buttons on that row have to keep working
/// or the demo is stuck with no way forward. When a worker IS running the
/// caller must go through its channel instead - two owners of the upload cache
/// would overwrite each other's entries.
pub async fn process_one(
    app: AppHandle,
    state: Arc<UploadState>,
    comps: Arc<CompsState>,
    api_base_url: String,
    token: String,
    path: PathBuf,
    enter: bool,
) -> Result<()> {
    let client = Client::new(api_base_url, token)?;
    let mut cache = UploadCache::load();
    let decision = if enter { CompsDecision::Enter } else { CompsDecision::Normal };
    handle_file(&client, &state, &comps, &app, &mut cache, path, decision).await;
    Ok(())
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
    comps: Arc<CompsState>,
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
                handle_file(&client, &state, &comps, &app, &mut cache, path, CompsDecision::Guard).await;
            }
            Message::CompsEnter(path) => {
                handle_file(&client, &state, &comps, &app, &mut cache, path, CompsDecision::Enter).await;
            }
            Message::CompsUploadNormally(path) => {
                handle_file(&client, &state, &comps, &app, &mut cache, path, CompsDecision::Normal).await;
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
                    handle_file(&client, &state, &comps, &app, &mut cache, p, CompsDecision::Guard).await;
                }
            }
            Message::ResetCacheStatuses => {
                cache.reset_statuses();
                let _ = cache.save();
            }
            Message::RescanFolder { folder, recursive } => {
                // Reopen the coalescing gate first: from here on a NEW rescan
                // request must queue again, because this walk can miss files
                // that appear while it is already past their directory.
                state.rescan_dequeued();
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
                            handle_file(&client, &state, &comps, &app, &mut cache, p.to_path_buf(), CompsDecision::Guard).await;
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
                            handle_file(&client, &state, &comps, &app, &mut cache, p, CompsDecision::Guard).await;
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
    comps: &Arc<CompsState>,
    app: &AppHandle,
    cache: &mut UploadCache,
    path: PathBuf,
    decision: CompsDecision,
) {
    // Normalise FIRST so every downstream key (the already-present short
    // circuit below, state.update's find-by-path, the queue rows we
    // insert, and what we persist to queue.json) uses the SAME form the
    // cache does. The watcher feeds verbatim `\\?\E:\…` paths on Windows
    // while walkdir / list_demos feed bare `E:\…`; left unnormalised the
    // two sides key the same file into different buckets, so a row could
    // never be matched back to its cache entry - the root cause of demos
    // stuck "Backing up 0/1" for a file already on the server.
    let path = crate::cache::normalize(&path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    // The two skips below are about the AUTOMATIC pass only. When the user
    // answers a held demo, that answer is about this file specifically and has
    // to reach the server even though the file has been seen before.
    if decision == CompsDecision::Guard {
        // Skip files already confirmed backed up earlier THIS session. The
        // session-wide `handled` set (unlike the QUEUE_CAP-bounded visible
        // queue) doesn't forget a file once it scrolls out of view, so the
        // periodic rescan no longer re-confirms - and re-counts - the whole
        // library every PERIODIC_RESCAN_SECS.
        if state.is_handled(&path) {
            return;
        }

        // A demo waiting on the user's answer stays waiting. Without this the
        // periodic rescan would walk back over it every half hour and re-ask a
        // question that is already on screen.
        if matches!(
            state.status_of(&path),
            Some(UploadStatus::HeldForComps) | Some(UploadStatus::CompsEntered)
        ) {
            return;
        }
    }

    let size_bytes = std::fs::metadata(&path).ok().map(|m| m.len());

    // Persistent cache: if the file's current size+mtime match what we
    // recorded last time we uploaded it, we don't need to re-hash or call
    // the server. Surface it as Done/Duplicate with reason="cache" so the
    // user can tell apart "we already uploaded this" from "the server
    // independently confirmed a hash match".
    //
    // Skipped when the user is deliberately entering the demo into comps: an
    // explicit answer has to reach the server, which is the only side that can
    // say whether the entry is a duplicate of something already there.
    if decision != CompsDecision::Enter {
        if let Some(entry) = cache.get_if_fresh(&path) {
            let status = match entry.status.as_str() {
                "done" => UploadStatus::Done,
                // A demo already entered into a round is on the server but not
                // public, and must not be reported as an ordinary backup.
                CACHE_STATUS_COMPS => UploadStatus::CompsEntered,
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
    }

    // The comps guard. Everything above this point is about files we have seen
    // before; from here the file is new, and the question is where it may go.
    let comps_target: Option<CompsHold> = match decision {
        // The user chose the ordinary path for this one file. The setting is
        // untouched - the next demo is asked about again.
        CompsDecision::Normal => None,
        // The user chose comps. Prefer the round the demo was held for over
        // whatever is open now: the two differ exactly while a week turns
        // over, and entering a run into the wrong round is not something the
        // user can undo.
        CompsDecision::Enter => state
            .comps_hold_of(&path)
            .or_else(|| comps.guard_match(&filename).map(CompsHold::from))
            .or_else(|| {
                comps.open_round_id().map(|round_id| CompsHold {
                    round_id,
                    comp_number: 0,
                    physics: String::new(),
                    map: String::new(),
                    submission_id: None,
                })
            }),
        CompsDecision::Guard => match comps.mode() {
            CompsMode::Off => None,
            _ => comps.guard_match(&filename).map(CompsHold::from),
        },
    };

    if decision == CompsDecision::Enter && comps_target.is_none() {
        state.update(app, &path, &filename, |u| {
            u.status = UploadStatus::Error;
            u.error = Some("No comps round is open right now.".to_string());
        });
        return;
    }

    // `ask` holds the file and shows the two buttons. Nothing is hashed and
    // nothing is sent - the demo simply stays where it is until answered.
    if decision == CompsDecision::Guard && comps.mode() == CompsMode::Ask {
        if let Some(hold) = comps_target.clone() {
            state.update(app, &path, &filename, |u| {
                u.status = UploadStatus::HeldForComps;
                u.size_bytes = size_bytes;
                u.error = None;
                u.comps = Some(hold);
            });
            return;
        }
    }

    state.update(app, &path, &filename, |u| {
        u.status = UploadStatus::Hashing;
        u.size_bytes = size_bytes;
        u.error = None;
        // The user sent this one down the ordinary route, so the row should
        // stop claiming it belongs to a round.
        if decision == CompsDecision::Normal {
            u.comps = None;
        }
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

    // Offline self-heal. get_if_fresh rejected this file on its size+mtime
    // freshness check, but if the hash we just computed still matches the
    // one we cached for it, the bytes are identical to what we already
    // uploaded - the file was merely touched (mtime drift from a backup
    // tool, antivirus, an editor re-save). Adopt the cached terminal
    // status instead of a needless server round-trip, and rewrite the
    // cache so the next rescan gets a clean freshness hit. This is what
    // lets a stuck row recover even with no network / not logged in.
    if decision != CompsDecision::Enter {
        if let Some((cstatus, cdemo_id)) = cache
            .get(&path)
            .filter(|e| {
                e.hash == md5
                    && (e.status == "done"
                        || e.status == "duplicate"
                        || e.status == CACHE_STATUS_COMPS)
            })
            .map(|e| (e.status.clone(), e.demo_id))
        {
            let status = match cstatus.as_str() {
                "done" => UploadStatus::Done,
                CACHE_STATUS_COMPS => UploadStatus::CompsEntered,
                _ => UploadStatus::Duplicate,
            };
            state.update(app, &path, &filename, |u| {
                u.status = status;
                u.demo_id = cdemo_id;
                u.duplicate_reason = Some("cache".to_string());
            });
            cache.insert(&path, md5, &cstatus, cdemo_id);
            let _ = cache.save();
            return;
        }
    }

    // The comps path forks here, BEFORE lookup-by-hash. Not for speed: the
    // ordinary path's early returns all end in a public upload, and a fork
    // placed after them would be one refactor away from letting a comps demo
    // through. The comps route does its own duplicate check server-side.
    if let Some(hold) = comps_target {
        let auto = decision != CompsDecision::Enter;
        submit_to_comps(client, state, app, cache, &path, &filename, &md5, hold, auto, size_bytes)
            .await;
        return;
    }

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

/// Send a demo to the comps round instead of to the public library.
///
/// `auto` says the launcher decided this from the filename rather than the
/// user picking it, and it travels to the server, which treats the two
/// differently: an auto entry that turns out not to be a run of the map is
/// withdrawn and left as an ordinary upload, while a hand-picked one is
/// refused where the user can see it.
///
/// A failure here does NOT fall back to the ordinary upload. The row goes back
/// to being held, carrying the reason, and the two buttons stay on screen. The
/// whole point of the guard is that this file does not travel the public route
/// by accident, and "the entry failed" is not a reason to publish a run.
#[allow(clippy::too_many_arguments)]
async fn submit_to_comps(
    client: &Client,
    state: &Arc<UploadState>,
    app: &AppHandle,
    cache: &mut UploadCache,
    path: &Path,
    filename: &str,
    md5: &str,
    hold: CompsHold,
    auto: bool,
    size_bytes: Option<u64>,
) {
    state.update(app, path, filename, |u| {
        u.status = UploadStatus::Uploading;
        u.error = None;
        u.comps = Some(hold.clone());
    });

    let t_up = Instant::now();
    let result = client.upload_comps_demo(path, hold.round_id, auto).await;
    let up_bps = size_bytes.and_then(|s| throughput_bps(s, t_up.elapsed()));

    match result {
        Ok(r) => {
            let entered = CompsHold { submission_id: Some(r.submission_id), ..hold };
            state.update(app, path, filename, |u| {
                u.status = UploadStatus::CompsEntered;
                u.demo_id = Some(r.demo_id);
                u.upload_throughput_bps = up_bps;
                u.comps = Some(entered.clone());
                u.error = None;
            });
            cache.insert(path, md5.to_string(), CACHE_STATUS_COMPS, Some(r.demo_id));
            let _ = cache.save();
        }
        // Already on the server - as an ordinary upload made before the round
        // opened, or as an earlier entry. Either way there is nothing to
        // retry, and no id comes back: comps does not answer whose demo it
        // collided with while the round is still running.
        Err(ApiError::AlreadyUploaded) => {
            state.update(app, path, filename, |u| {
                u.status = UploadStatus::Duplicate;
                u.duplicate_reason = Some("server".to_string());
                u.error = None;
            });
            cache.insert(path, md5.to_string(), "duplicate", None);
            let _ = cache.save();
        }
        Err(e) => {
            state.update(app, path, filename, |u| {
                u.status = UploadStatus::HeldForComps;
                u.error = Some(format!("{e}"));
            });
        }
    }
}
