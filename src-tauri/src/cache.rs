//! Persistent local cache of already-uploaded demos.
//!
//! Without this cache every launcher start re-hashes every `.dm_*` file
//! in the demos folder and calls `/api/launcher/lookup-by-hash` for each
//! one. For 500 demos on SSD that's roughly 50s hashing + 75s of HTTP
//! round-trips = ~2 minutes of work to discover "everything is already
//! uploaded". The cache cuts that to a directory listing.
//!
//! Invalidation: cache hit requires the file's current (size, mtime) to
//! match the cached values. Either differs and we fall through to the
//! full hash + lookup path. This is intentionally a best-effort speed-
//! up - the server-side dedup logic is still authoritative, so a stale
//! cache entry can only cause an extra round-trip, never a wrong upload.
//!
//! Atomicity: writes go through a .tmp file + rename so a crash mid-save
//! can't leave a corrupted JSON behind. Load tolerates missing or
//! unparseable files by returning Default::default() - we never want a
//! flaky cache file to break the watcher.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedEntry {
    /// Unix epoch seconds. We trust this for invalidation; if the
    /// filesystem's mtime is unreliable (network share, antivirus
    /// touching files), the user can hit "Force re-check" in Settings.
    pub mtime: u64,
    pub size: u64,
    pub hash: String,
    /// "done" | "duplicate" - both mean "the server has it", which is
    /// all we care about for skip-on-rescan.
    pub status: String,
    pub demo_id: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UploadCache {
    pub files: HashMap<PathBuf, CachedEntry>,
}

impl UploadCache {
    /// Co-located with config.json so a single config_dir wipe (e.g.
    /// uninstall) clears both. Errors here mean we can't even resolve
    /// the user's app-data dir, which is exotic enough to surface up.
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
            .context("could not resolve platform config directory")?;
        let dir = dirs.config_dir().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
        Ok(dir.join("uploaded.json"))
    }

    /// Best-effort load: any error (missing file, parse failure,
    /// permissions) returns an empty cache so the watcher just falls
    /// back to full hash+lookup, the conservative behavior.
    pub fn load() -> Self {
        let Ok(path) = Self::path() else { return Self::default() };
        let Ok(raw) = fs::read_to_string(&path) else { return Self::default() };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let raw = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }

    /// Wipe the cache from disk. Called from the "Force re-check" UI
    /// button when the user wants the next rescan to re-verify every
    /// file against the server (e.g. after an admin deleted a demo
    /// server-side and the user wants to re-upload).
    pub fn clear() -> Result<()> {
        let path = Self::path()?;
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove {:?}", path))?;
        }
        Ok(())
    }

    /// Returns Some(entry) only if the entry exists AND its recorded
    /// (size, mtime) match the file at `path` right now. Any mismatch
    /// is treated as a cache miss; the caller will re-hash and either
    /// upload or confirm-duplicate, and overwrite the cache entry.
    pub fn get_if_fresh(&self, path: &Path) -> Option<&CachedEntry> {
        let entry = self.files.get(path)?;
        let meta = fs::metadata(path).ok()?;
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();
        if entry.size == size && entry.mtime == mtime {
            Some(entry)
        } else {
            None
        }
    }

    /// Look up by exact path. Used by the demos-library command to
    /// pair a filesystem entry with its known hash + demo_id without
    /// going through the freshness check (so a file we've previously
    /// uploaded still shows its demo_id in the library even if it
    /// got touched in some way after).
    pub fn get(&self, path: &Path) -> Option<&CachedEntry> {
        self.files.get(path)
    }

    /// Record a successful upload (or confirmed-duplicate) for the
    /// given file. Caller passes the freshly-computed hash + the
    /// server's response. mtime/size are read from disk here so the
    /// stored values reflect the state we actually saw + hashed.
    pub fn insert(
        &mut self,
        path: &Path,
        hash: String,
        status: &str,
        demo_id: Option<u64>,
    ) {
        let Ok(meta) = fs::metadata(path) else { return };
        let size = meta.len();
        let Ok(mtime_st) = meta.modified() else { return };
        let Ok(mtime_dur) = mtime_st.duration_since(SystemTime::UNIX_EPOCH) else { return };
        let mtime = mtime_dur.as_secs();
        self.files.insert(
            path.to_path_buf(),
            CachedEntry {
                mtime,
                size,
                hash,
                status: status.to_string(),
                demo_id,
            },
        );
    }
}
