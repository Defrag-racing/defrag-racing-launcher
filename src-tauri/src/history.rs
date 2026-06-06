//! Persistent log of `defrag://` connections - "where did I join and
//! when" for the launcher.
//!
//! Each successful defrag:// click (auto-connect or user-confirmed)
//! appends an entry with timestamp + IP:port + optional map / server
//! name / physics fetched at click time. Frontend renders the latest
//! N entries in the History tab so the user can find a server they
//! played 3 days ago without remembering its IP.
//!
//! Persisted to history.json next to config.json / queue.json /
//! uploaded.json so a single config_dir wipe clears everything in one
//! go. Atomic .tmp + rename on write so a crash mid-save can't corrupt
//! the file; best-effort load tolerates missing or unparseable files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Soft cap on history depth. New entries push older ones out the back
/// once we hit this. 200 covers a power user with a few connects per
/// day for a few months; older history we don't need to keep.
const HISTORY_CAP: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionEntry {
    /// Stable session id, generated at log time as
    /// `{timestamp_ms}-{ip}-{port}`. Lets the map tracker append maps to
    /// the right entry while the engine process is alive, and the frontend
    /// key its expandable rows. Defaults to empty for legacy entries
    /// written before this field existed.
    #[serde(default)]
    pub id: String,
    /// Unix-epoch milliseconds at click time. Frontend renders this
    /// with the user's locale; backend doesn't care about formatting.
    pub timestamp_ms: u64,
    pub ip: String,
    pub port: u16,
    /// Best-effort enrichment from the live server lookup at click
    /// time. None when the frontend wasn't involved (auto-connect
    /// path) or when the server wasn't on the live list (private /
    /// off-list / token absent).
    #[serde(default)]
    pub map: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub physics: Option<String>,
    /// "auto" when the auto-connect Settings toggle was on and the
    /// launcher launched the engine directly, "confirmed" when the
    /// user pressed Connect on the banner. Lets the History view
    /// distinguish the two cases. Defaults to "confirmed" for legacy
    /// entries.
    #[serde(default = "default_source")]
    pub source: String,
    /// Maps that played on this server while the launched engine process
    /// was alive (the per-session map history). Appended in the background
    /// by the session tracker as the server rotates maps; empty when no
    /// changes were seen (no token, off-list server, or the game closed
    /// before a rotation).
    #[serde(default)]
    pub maps_played: Vec<MapPlay>,
}

/// One map the server was running, observed during a connected session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapPlay {
    pub map: String,
    /// Unix-epoch milliseconds when we first observed this map on the
    /// server during the session.
    pub timestamp_ms: u64,
    #[serde(default)]
    pub physics: Option<String>,
}

fn default_source() -> String { "confirmed".to_string() }

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HistoryFile {
    #[serde(default)]
    pub entries: Vec<ConnectionEntry>,
}

/// In-memory history wrapped in a Mutex so concurrent log() calls (one
/// from the deep-link handler thread, one from the Tauri command
/// thread for confirmed connects) can't race on the file.
pub struct ConnectionHistory {
    inner: Mutex<HistoryFile>,
}

impl Default for ConnectionHistory {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Self::load_file().unwrap_or_default()),
        }
    }
}

impl ConnectionHistory {
    fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
            .context("could not resolve platform config directory")?;
        let dir = dirs.config_dir().to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
        Ok(dir.join("history.json"))
    }

    /// Best-effort load: missing / unreadable / unparseable file
    /// returns an empty history. We never want a flaky history file to
    /// break defrag:// handling.
    fn load_file() -> Result<HistoryFile> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(HistoryFile::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    fn save_file(file: &HistoryFile) -> Result<()> {
        let path = Self::path()?;
        let raw = serde_json::to_string_pretty(file)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }

    /// Prepend a new entry, cap to HISTORY_CAP, persist. The
    /// timestamp is filled in here (caller doesn't need to be a clock).
    /// Errors during persist are logged but not surfaced - history is
    /// nice-to-have, never block a connect on a failed file write.
    pub fn log(
        &self,
        ip: String,
        port: u16,
        map: Option<String>,
        server_name: Option<String>,
        physics: Option<String>,
        source: &str,
    ) -> String {
        let ts = now_ms();
        let id = format!("{}-{}-{}", ts, ip, port);
        // Seed the map timeline with the map we already know at connect
        // time (if any), so the session starts with the map you joined on
        // rather than only logging the first rotation after you arrive.
        let maps_played = match (&map, &physics) {
            (Some(m), p) => vec![MapPlay { map: m.clone(), timestamp_ms: ts, physics: p.clone() }],
            _ => Vec::new(),
        };
        let entry = ConnectionEntry {
            id: id.clone(),
            timestamp_ms: ts,
            ip,
            port,
            map,
            server_name,
            physics,
            source: source.to_string(),
            maps_played,
        };
        let mut guard = self.inner.lock().unwrap();
        guard.entries.insert(0, entry);
        if guard.entries.len() > HISTORY_CAP {
            guard.entries.truncate(HISTORY_CAP);
        }
        if let Err(e) = Self::save_file(&guard) {
            crate::log_startup(&format!("history.log: save failed: {}", e));
        }
        id
    }

    /// Append a map to a session's timeline, found by entry id. No-op if
    /// the entry has aged out of the cap, or if the map is identical to the
    /// last one already recorded for it (server still on the same map).
    /// Persisted best-effort like log().
    pub fn append_map(&self, session_id: &str, map: String, physics: Option<String>, ts_ms: u64) {
        let mut guard = self.inner.lock().unwrap();
        let Some(entry) = guard.entries.iter_mut().find(|e| e.id == session_id) else {
            return;
        };
        if entry.maps_played.last().map(|m| m.map.as_str()) == Some(map.as_str()) {
            return;
        }
        entry.maps_played.push(MapPlay { map, timestamp_ms: ts_ms, physics });
        if let Err(e) = Self::save_file(&guard) {
            crate::log_startup(&format!("history.append_map: save failed: {}", e));
        }
    }

    /// Snapshot the current entries for the frontend. Cheap clone,
    /// vector is bounded at HISTORY_CAP.
    pub fn list(&self) -> Vec<ConnectionEntry> {
        self.inner.lock().unwrap().entries.clone()
    }

    /// Wipe history from memory and from disk. Used by the
    /// "Clear history" button in the History view.
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.entries.clear();
        let _ = Self::save_file(&guard);
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
