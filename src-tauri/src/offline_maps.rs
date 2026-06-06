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

use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;

#[derive(Serialize)]
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

const LEVELSHOT_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "tga"];

fn baseq3_dir(engine_path: &Path) -> Option<PathBuf> {
    engine_path.parent().map(|d| d.join("baseq3"))
}

/// List every `maps/*.bsp` across the pk3s in baseq3, each paired with the
/// pk3 it came from and whether it has a levelshot.
pub fn list(engine_path: &Path) -> Result<Vec<OfflineMap>> {
    let baseq3 = baseq3_dir(engine_path).context("engine path has no parent")?;
    let mut out: Vec<OfflineMap> = Vec::new();

    let rd = match std::fs::read_dir(&baseq3) {
        Ok(rd) => rd,
        Err(_) => return Ok(out), // no baseq3 yet -> empty list, not an error
    };

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
            out.push(OfflineMap {
                has_levelshot: levelshots.contains(&stem),
                name: disp,
                pk3: pk3_name.clone(),
                pk3_path: pk3_path.clone(),
            });
        }
    }

    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    Ok(out)
}

fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("racing", "defrag", "launcher")
        .map(|d| d.cache_dir().join("mapthumbs"))
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
