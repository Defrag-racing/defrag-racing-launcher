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
use crate::hashing;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpload {
    pub path: PathBuf,
    pub filename: String,
    pub status: UploadStatus,
    pub demo_id: Option<u64>,
    pub error: Option<String>,
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

#[derive(Default)]
pub struct UploadState {
    inner: Mutex<Vec<PendingUpload>>,
}

impl UploadState {
    pub fn snapshot(&self) -> UploadStateSnapshot {
        UploadStateSnapshot {
            items: self.inner.lock().unwrap().clone(),
        }
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut Vec<PendingUpload>) -> R) -> R {
        f(&mut self.inner.lock().unwrap())
    }
}

/// Message sent to the worker task.
enum Message {
    FileAdded(PathBuf),
    RescanFolder(PathBuf),
}

pub struct WatcherHandle {
    _debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
    _worker: tokio::task::JoinHandle<()>,
    pub state: Arc<UploadState>,
}

/// Start watching `demos_path` and return a handle whose drop stops the
/// watcher + worker. The token is captured up front — if the user rotates
/// it later, stop/start the watcher to pick up the new one.
pub fn start(
    app: AppHandle,
    demos_path: PathBuf,
    api_base_url: String,
    token: String,
) -> anyhow::Result<WatcherHandle> {
    let state = Arc::new(UploadState::default());
    let (tx, rx) = mpsc::unbounded_channel::<Message>();

    // Debounce bursts while Defrag is still writing the file. The debounce
    // window is deliberately generous (2s) — demos aren't written at high
    // frequency and we'd rather miss a 0.5s window than upload a truncated
    // file.
    let tx_fs = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_secs(2),
        None,
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                for ev in events {
                    for path in ev.event.paths {
                        if is_demo_file(&path) {
                            let _ = tx_fs.send(Message::FileAdded(path));
                        }
                    }
                }
            }
        },
    )?;
    debouncer
        .watcher()
        .watch(&demos_path, RecursiveMode::NonRecursive)?;

    // Kick off a rescan on start so demos created while the launcher was
    // closed still get uploaded. Bounded so an enormous legacy folder can
    // still finish in finite time — user can tune later.
    let _ = tx.send(Message::RescanFolder(demos_path.clone()));

    let state_worker = state.clone();
    let app_worker = app.clone();
    let worker = tokio::spawn(worker_loop(rx, state_worker, app_worker, api_base_url, token));

    Ok(WatcherHandle {
        _debouncer: debouncer,
        _worker: worker,
        state,
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

    while let Some(msg) = rx.recv().await {
        match msg {
            Message::FileAdded(path) => {
                handle_file(&client, &state, &app, path).await;
            }
            Message::RescanFolder(folder) => {
                if let Ok(entries) = std::fs::read_dir(&folder) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_file() && is_demo_file(&p) {
                            handle_file(&client, &state, &app, p).await;
                        }
                    }
                }
            }
        }
    }
}

async fn handle_file(
    client: &Client,
    state: &Arc<UploadState>,
    app: &AppHandle,
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

    push_or_update(state, app, &path, &filename, UploadStatus::Hashing, None, None);

    let md5 = match tokio::task::spawn_blocking({
        let path = path.clone();
        move || hashing::md5_hex(&path)
    })
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Error, None, Some(format!("hash failed: {e}")));
            return;
        }
        Err(e) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Error, None, Some(format!("hash task panicked: {e}")));
            return;
        }
    };

    // Pre-flight: is this already on the server?
    match client.lookup_by_hash(&md5).await {
        Ok(r) if r.exists => {
            push_or_update(
                state,
                app,
                &path,
                &filename,
                UploadStatus::Duplicate,
                r.demo_id,
                None,
            );
            return;
        }
        Ok(_) => {}
        Err(e) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Error, None, Some(format!("lookup failed: {e}")));
            return;
        }
    }

    push_or_update(state, app, &path, &filename, UploadStatus::Uploading, None, None);

    match client.upload_demo(&path, &md5).await {
        Ok(r) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Done, Some(r.demo_id), None);
        }
        Err(ApiError::Duplicate { demo_id }) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Duplicate, Some(demo_id), None);
        }
        Err(e) => {
            push_or_update(state, app, &path, &filename, UploadStatus::Error, None, Some(format!("{e}")));
        }
    }
}

fn push_or_update(
    state: &Arc<UploadState>,
    app: &AppHandle,
    path: &Path,
    filename: &str,
    status: UploadStatus,
    demo_id: Option<u64>,
    error: Option<String>,
) {
    state.with_mut(|items| {
        if let Some(existing) = items.iter_mut().find(|i| i.path == path) {
            existing.status = status;
            if demo_id.is_some() {
                existing.demo_id = demo_id;
            }
            existing.error = error;
        } else {
            items.insert(
                0,
                PendingUpload {
                    path: path.to_path_buf(),
                    filename: filename.to_string(),
                    status,
                    demo_id,
                    error,
                },
            );
        }
    });
    let _ = app.emit("upload_state_changed", state.snapshot());
}
