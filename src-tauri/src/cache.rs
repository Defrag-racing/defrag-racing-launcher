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

    /// Best-effort load: missing file or permission error returns an
    /// empty cache so the watcher just falls back to full hash+lookup,
    /// the conservative behavior.
    ///
    /// If the file exists but FAILS to parse (corruption, partial
    /// write, encoding glitch), we rename it to `uploaded.json.bak.<ts>`
    /// before returning empty. The previous behavior silently dropped
    /// the parse error and the very next `cache.save()` overwrote the
    /// corrupted-but-still-readable JSON with a fresh empty cache -
    /// users lost thousands of cached hashes that way, with no path
    /// to recovery. Renaming preserves the file for manual inspection
    /// + restore.
    ///
    /// Path normalisation runs over every loaded key. Without this,
    /// cache entries written by the watcher (which receives
    /// notify-debouncer paths) and lookups from list_demos (which
    /// walkdirs the configured demos_path) can disagree on whether
    /// the Windows verbatim `\\?\` prefix is present, so `E:\foo`
    /// and `\\?\E:\foo` resolve to different HashMap buckets even
    /// though they point at the same file. Migration is one-shot per
    /// process: the next save() writes the cleaned keys back to disk.
    pub fn load() -> Self {
        let Ok(path) = Self::path() else { return Self::default() };
        let Ok(raw) = fs::read_to_string(&path) else { return Self::default() };
        let mut cache: UploadCache = match serde_json::from_str(&raw) {
            Ok(c) => c,
            Err(e) => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let bak = path.with_file_name(format!("uploaded.json.bak.{}", ts));
                let _ = fs::rename(&path, &bak);
                crate::log_startup(&format!(
                    "uploaded.json parse failed ({}); renamed to {:?}",
                    e, bak
                ));
                return UploadCache::default();
            }
        };
        // Migrate any prefixed / dirty keys into their normalised form.
        // Collecting first so we don't mutate the map mid-iteration.
        let dirty: Vec<PathBuf> = cache
            .files
            .keys()
            .filter(|k| normalize(k) != **k)
            .cloned()
            .collect();
        for old in dirty {
            if let Some(entry) = cache.files.remove(&old) {
                cache.files.insert(normalize(&old), entry);
            }
        }
        cache
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
        let entry = self.files.get(&normalize(path))?;
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
        self.files.get(&normalize(path))
    }

    /// Drop the cache entry for a single path. Called when the user
    /// deletes a demo from the Library context menu - we don't want
    /// the stale hash to come back via a subsequent rescan and re-
    /// upload an empty path. Best-effort: missing entry / missing
    /// file is fine, the goal is just "forget this".
    pub fn remove(&mut self, path: &Path) {
        self.files.remove(&normalize(path));
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
            normalize(path),
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

/// Strip Windows verbatim-path prefix `\\?\` so callers feeding paths
/// from notify-debouncer (which canonicalises on Windows) and walkdir
/// (which doesn't) hit the same HashMap bucket. On macOS / Linux the
/// prefix never appears so this is a no-op there. We deliberately do
/// NOT lowercase: Windows filenames are case-insensitive but Rust's
/// PathBuf comparison treats them as case-sensitive, and forcing
/// lowercase would re-introduce mismatch risk on case-preserving
/// filesystems (ReFS, exFAT external drives).
pub fn normalize(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}
