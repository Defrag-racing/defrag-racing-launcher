//! Offline Maps tab backend: list the maps installed in the engine's
//! baseq3 folder and pull their thumbnails out of the pk3s.
//!
//! A map = a `maps/<name>.bsp` entry inside a pk3 (the Quake3 engine
//! convention; one pk3 can hold several). A thumbnail = the levelshot at
//! `levelshots/<name>.{jpg,jpeg,png,tga}`. We only read the zip directory
//! (for names) and pull the single small levelshot file - never unpack the
//! whole map. The levelshot search + TGA handling mirror the website's
//! `maps:extract-levelshots` command, so the offline grid matches online.
//!
//! Thumbnails come back as base64 data URLs (the webview can't read
//! arbitrary local files without asset-protocol scoping) and are cached on
//! disk so re-opening the tab doesn't re-read the pk3s.
//!
//! ## Performance
//!
//! Opening every pk3 in baseq3 to read its zip directory is the expensive
//! part - a defrag player can have thousands of pk3s, and re-scanning them
//! on every tab open pins the disk at 100%. So the full scan runs **once**
//! and is cached to a manifest on disk (`offline_maps-<key>.json`). Re-opening
//! the tab only does a cheap metadata pass (read_dir + file len/mtime) to build
//! a signature; if it matches the cached manifest's signature we reuse the
//! cached map list and never touch a single zip. The frontend additionally
//! paginates, so only a page worth of thumbnails is ever extracted.
//!
//! The manifest is **per install**, and the install path is normalised before
//! it becomes the key. Both matter, and neither used to be true: there was one
//! manifest for every root, keyed by the path spelling it was built with. The
//! Maps tab passed the engine path from the config (`\\?\D:\...`) while the
//! demo player's map check passed a path derived from a demo (`D:\...`) - the
//! same folder, two spellings, so each call saw a manifest "for a different
//! install", rescanned every pk3, and overwrote the other's cache. With 555
//! pk3s that is seconds of disk on every single demo played.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct OfflineMap {
    /// Map name = the bsp basename (what you pass to `+vq3` / `+cpm`).
    pub name: String,
    /// pk3 filename the map lives in (basename).
    pub pk3: String,
    /// Absolute path to that pk3 (passed back for thumbnail extraction).
    pub pk3_path: String,
    /// Whether a levelshot for this map exists in the pk3 (so the UI knows
    /// whether to bother requesting a thumbnail).
    pub has_levelshot: bool,
}

/// One page of offline maps for the paginated tab. Mirrors the shape of the
/// online maps endpoint so the frontend can reuse the same pager.
#[derive(Serialize)]
pub struct OfflineMapPage {
    pub data: Vec<OfflineMap>,
    pub total: usize,
    pub current_page: u32,
    pub last_page: u32,
    pub per_page: u32,
}

const LEVELSHOT_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "tga"];

/// Game dirs the engine searches for pk3s. The engine always adds the
/// basegame (`baseq3`) regardless of `fs_game`, plus the active mod dir
/// (`defrag`) - see oDFe `FS_Startup` (code/qcommon/files.c). Defrag's own
/// map auto-download writes into the `defrag` folder, so maps live in both;
/// scanning only baseq3 (the old behaviour) missed every map under defrag.
/// `fs_game` is passed on the command line (not saved to config), and this
/// is a defrag-only launcher, so the candidate mod dir is fixed to "defrag".
///
/// Existing game directories under the engine's install folder, in engine
/// search-priority order (mod dir wins over basegame, so list it first).
fn game_dirs(engine_path: &Path) -> Vec<PathBuf> {
    let Some(install) = engine_path.parent() else { return Vec::new() };
    // defrag before baseq3 so a map present in both is listed from the dir
    // the engine would actually load it from.
    ["defrag", "baseq3"]
        .iter()
        .map(|d| install.join(d))
        .filter(|p| p.is_dir())
        .collect()
}

/// Cheap change-detection signature across all game dirs: the sorted set of
/// pk3 (dir, name, size, mtime). Metadata only - no zip is opened - so this
/// is fast even with thousands of files and lets us skip the expensive scan
/// when nothing changed since the cached manifest was built.
fn signature(dirs: &[PathBuf]) -> String {
    let mut entries: Vec<(String, String, u64, u64)> = Vec::new();
    for dir in dirs {
        let dir_key = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            let is_pk3 = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pk3"))
                .unwrap_or(false);
            if !is_pk3 {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let (len, mtime) = ent
                .metadata()
                .map(|md| {
                    let mtime = md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (md.len(), mtime)
                })
                .unwrap_or((0, 0));
            entries.push((dir_key.clone(), name, len, mtime));
        }
    }
    entries.sort();
    let mut h = DefaultHasher::new();
    entries.hash(&mut h);
    format!("{:x}", h.finish())
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    /// Install dir this manifest describes (so switching engines rebuilds).
    install: String,
    /// signature() at build time.
    signature: String,
    maps: Vec<OfflineMap>,
}

/// Filename prefix, shared by the per-install manifests and by `clear_cache`.
const MANIFEST_PREFIX: &str = "offline_maps-";

/// One manifest per install, so two roots can both stay cached instead of
/// evicting each other. The install path is hashed rather than used raw: it
/// contains separators and colons, which have no business in a filename.
fn manifest_path(install: &str) -> Option<PathBuf> {
    let mut h = DefaultHasher::new();
    install.hash(&mut h);
    directories::ProjectDirs::from("racing", "defrag", "launcher")
        .map(|d| d.cache_dir().join(format!("{MANIFEST_PREFIX}{:x}.json", h.finish())))
}

fn load_manifest(install: &str) -> Option<Manifest> {
    let path = manifest_path(install)?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_manifest(m: &Manifest) {
    let Some(path) = manifest_path(&m.install) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(m) {
        let _ = std::fs::write(path, bytes);
    }
}

/// The expensive scan: open every pk3 across the game dirs and read its zip
/// directory to collect `maps/*.bsp` names + which have levelshots. Runs only
/// on a cache miss. A pk3 with no `maps/*.bsp` entry contributes nothing (a
/// texture/sound/config pack is naturally ignored). Maps are deduplicated by
/// name across dirs - `dirs` is in engine priority order, so the first
/// occurrence (the one the engine would load) wins.
fn scan(dirs: &[PathBuf]) -> Vec<OfflineMap> {
    let mut out: Vec<OfflineMap> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            let is_pk3 = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pk3"))
                .unwrap_or(false);
            if !is_pk3 {
                continue;
            }
            let Ok(file) = File::open(&path) else { continue };
            let Ok(zip) = zip::ZipArchive::new(file) else { continue };

            // (display-case name, lowercase stem) for each map; lowercase
            // levelshot stems present in the pk3.
            let mut maps: Vec<(String, String)> = Vec::new();
            let mut levelshots: HashSet<String> = HashSet::new();

            for raw in zip.file_names() {
                let lower = raw.to_ascii_lowercase();
                if let Some(stem) = lower.strip_prefix("maps/").and_then(|r| r.strip_suffix(".bsp")) {
                    if !stem.is_empty() && !stem.contains('/') {
                        // keep the original-case basename for display
                        let disp = Path::new(raw)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(stem)
                            .to_string();
                        maps.push((disp, stem.to_string()));
                    }
                } else if let Some(rest) = lower.strip_prefix("levelshots/") {
                    if let Some(dot) = rest.rfind('.') {
                        let stem = &rest[..dot];
                        let ext = &rest[dot + 1..];
                        if !stem.contains('/') && LEVELSHOT_EXTS.contains(&ext) {
                            levelshots.insert(stem.to_string());
                        }
                    }
                }
            }

            let pk3_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let pk3_path = path.to_string_lossy().to_string();
            for (disp, stem) in maps {
                // dedup by map name across dirs (engine loads one of them)
                if !seen.insert(stem.clone()) {
                    continue;
                }
                out.push(OfflineMap {
                    has_levelshot: levelshots.contains(&stem),
                    name: disp,
                    pk3: pk3_name.clone(),
                    pk3_path: pk3_path.clone(),
                });
            }
        }
    }

    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out
}

/// Full list of every `maps/*.bsp` across the pk3s in the engine's game dirs
/// (baseq3 + defrag). Served from the on-disk manifest when nothing changed;
/// only a genuine change (added/removed/modified pk3 in either dir) triggers
/// the heavy scan.
pub fn list(engine_path: &Path) -> Result<Vec<OfflineMap>> {
    // Normalised, or the same folder spelled two ways is two caches. See the
    // module note - this is what made every demo rescan the whole collection.
    let install = crate::cache::normalize(engine_path.parent().context("engine path has no parent")?)
        .to_string_lossy()
        .to_string();
    let dirs = game_dirs(engine_path);
    let sig = signature(&dirs);

    if let Some(m) = load_manifest(&install) {
        if m.install == install && m.signature == sig {
            return Ok(m.maps);
        }
    }

    let maps = scan(&dirs);
    save_manifest(&Manifest {
        install,
        signature: sig,
        maps: maps.clone(),
    });
    Ok(maps)
}

/// Paginated + name-filtered view over `list()`. The heavy work is the
/// cached `list()`; filtering + slicing here is in-memory and cheap.
pub fn list_paged(engine_path: &Path, page: u32, per_page: u32, search: &str) -> Result<OfflineMapPage> {
    let all = list(engine_path)?;
    let q = search.trim().to_ascii_lowercase();
    let filtered: Vec<OfflineMap> = if q.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|m| m.name.to_ascii_lowercase().contains(&q))
            .collect()
    };

    let per_page = per_page.max(1);
    let total = filtered.len();
    let last_page = ((total as u32).div_ceil(per_page)).max(1);
    let page = page.clamp(1, last_page);
    let start = ((page - 1) * per_page) as usize;
    let data: Vec<OfflineMap> = filtered.into_iter().skip(start).take(per_page as usize).collect();

    Ok(OfflineMapPage {
        data,
        total,
        current_page: page,
        last_page,
        per_page,
    })
}

fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("racing", "defrag", "launcher")
        .map(|d| d.cache_dir().join("mapthumbs"))
}

/// Remove everything the offline Maps tab caches on disk: the thumbnail
/// cache and every scan manifest. Called from the Settings "Reset launcher"
/// flow so a reset really starts from zero.
///
/// Sweeps by prefix rather than deleting one known filename: there is a
/// manifest per install, and the pre-0.1.48 single `offline_maps.json` may
/// still be lying around too.
pub fn clear_cache() {
    if let Some(dir) = cache_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
    let Some(root) = directories::ProjectDirs::from("racing", "defrag", "launcher")
        .map(|d| d.cache_dir().to_path_buf())
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(MANIFEST_PREFIX) || name == "offline_maps.json" {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn cache_key(pk3_path: &Path, map_name: &str) -> String {
    let pk3 = pk3_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect()
    };
    format!("{}__{}", safe(pk3), safe(&map_name.to_ascii_lowercase()))
}

fn to_data_url(mime: &str, bytes: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}

/// Extract a map's levelshot from its pk3 and return it as a data URL.
/// TGA levelshots are re-encoded to PNG (webview can't show TGA). Returns
/// Ok(None) when the pk3 has no levelshot for this map. Results are cached
/// on disk keyed by pk3 + map name.
pub fn thumb_data_url(pk3_path: &Path, map_name: &str) -> Result<Option<String>> {
    let key = cache_key(pk3_path, map_name);
    if let Some(dir) = cache_dir() {
        for ext in ["png", "jpg"] {
            let cp = dir.join(format!("{key}.{ext}"));
            if let Ok(bytes) = std::fs::read(&cp) {
                let mime = if ext == "png" { "image/png" } else { "image/jpeg" };
                return Ok(Some(to_data_url(mime, &bytes)));
            }
        }
    }

    let file = File::open(pk3_path).with_context(|| format!("open {}", pk3_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("read pk3 as zip")?;

    // Locate the levelshot entry for this map (case-insensitive), trying
    // the extensions in the same order the website does.
    let target = map_name.to_ascii_lowercase();
    let mut entry_name: Option<String> = None;
    let mut ext: Option<String> = None;
    for raw in zip.file_names() {
        let lower = raw.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("levelshots/") {
            if let Some(dot) = rest.rfind('.') {
                let (stem, e) = (&rest[..dot], &rest[dot + 1..]);
                if stem == target && LEVELSHOT_EXTS.contains(&e) {
                    entry_name = Some(raw.to_string());
                    ext = Some(e.to_string());
                    break;
                }
            }
        }
    }
    let (Some(entry_name), Some(ext)) = (entry_name, ext) else {
        return Ok(None);
    };

    let mut data = Vec::new();
    zip.by_name(&entry_name)
        .context("read levelshot entry")?
        .read_to_end(&mut data)
        .context("extract levelshot")?;

    let (mime, out_bytes, cache_ext): (&str, Vec<u8>, &str) = if ext == "tga" {
        let img = image::load_from_memory_with_format(&data, image::ImageFormat::Tga)
            .context("decode tga levelshot")?;
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .context("encode png")?;
        ("image/png", buf.into_inner(), "png")
    } else if ext == "png" {
        ("image/png", data, "png")
    } else {
        ("image/jpeg", data, "jpg")
    };

    if let Some(dir) = cache_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{key}.{cache_ext}")), &out_bytes);
    }

    Ok(Some(to_data_url(mime, &out_bytes)))
}
