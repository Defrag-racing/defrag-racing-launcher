//! Best-effort scan for oDFe / iDFe installations across the filesystem.
//!
//! The results are **suggestions**, never auto-applied. The UI shows the
//! whole list + a "Browse..." option so the user always picks explicitly.
//! This sidesteps the classic trap of grabbing the wrong binary from an
//! old install and silently breaking defrag:// deep-links later.
//!
//! Platform strategy:
//!   - Linux: /opt, ~/Games, ~/.local/share, ~/.steam/steam/steamapps/common,
//!     plus any "oDFe" / "iDFe" on $PATH (covers Flatpak symlinks too).
//!   - Windows: %ProgramFiles%, %ProgramFiles(x86)%, %LOCALAPPDATA%\Programs,
//!     Steam registry lookup → steamapps\common\Quake 3 Arena\.
//!   - macOS: /Applications, ~/Applications, ~/Library/Application Support/
//!     Steam/steamapps/common.
//!
//! We cap walkdir at depth 4 so we don't chew through the whole home
//! directory on a user with lots of cloud-synced folders.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineCandidate {
    pub kind: EngineKind,
    pub path: PathBuf,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    ODfe,
    IDfe,
    Other,
}

const MAX_DEPTH: usize = 4;

pub fn detect() -> Vec<EngineCandidate> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for root in candidate_roots() {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&root)
            .max_depth(MAX_DEPTH)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            if let Some(kind) = classify(name) {
                // Canonicalize so two roots pointing at the same install
                // (symlink + real path) don't both appear.
                let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                if seen.insert(canonical.clone()) {
                    out.push(EngineCandidate {
                        kind,
                        display_name: humanize(&canonical, kind),
                        path: canonical,
                    });
                }
            }
        }
    }

    // $PATH scan — catches Flatpak shims and manual ~/.local/bin installs.
    // Walk each PATH entry and let `classify` decide, so build-variant
    // suffixes (oDFe.vk.x64.exe etc) work the same as directory scans.
    if let Ok(path_var) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep).map(Path::new) {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = match name.to_str() {
                    Some(n) => n,
                    None => continue,
                };
                if let Some(kind) = classify(name_str) {
                    let p = entry.path();
                    let canonical = p.canonicalize().unwrap_or(p.clone());
                    if seen.insert(canonical.clone()) {
                        out.push(EngineCandidate {
                            kind,
                            display_name: humanize(&canonical, kind),
                            path: canonical,
                        });
                    }
                }
            }
        }
    }

    out
}

fn classify(filename: &str) -> Option<EngineKind> {
    // Only executables count — skip readmes, configs, screenshot folders, etc.
    let lower = filename.to_ascii_lowercase();
    let looks_executable = lower.ends_with(".exe")
        || lower.ends_with(".x86_64")
        || lower.ends_with(".x64")
        || lower.ends_with(".app")
        || !lower.contains('.'); // bare Unix binary

    if !looks_executable {
        return None;
    }

    // Prefix match: oDFe ships as oDFe.x64.exe, oDFe.vk.x64.exe,
    // oDFe.x86_64 on Linux, etc. Same idea for iDFe. Matching the prefix
    // catches current + future build variants without a whitelist.
    if lower.starts_with("odfe") {
        Some(EngineKind::ODfe)
    } else if lower.starts_with("idfe") {
        Some(EngineKind::IDfe)
    } else {
        None
    }
}

fn humanize(path: &Path, kind: EngineKind) -> String {
    let label = match kind {
        EngineKind::ODfe => "oDFe",
        EngineKind::IDfe => "iDFe",
        EngineKind::Other => "engine",
    };
    format!("{} at {}", label, path.display())
}

fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());

    #[cfg(target_os = "linux")]
    {
        roots.push(PathBuf::from("/opt"));
        roots.push(PathBuf::from("/usr/games"));
        if let Some(h) = &home {
            roots.push(h.join("Games"));
            roots.push(h.join(".local").join("share"));
            roots.push(h.join(".local").join("bin"));
            roots.push(h.join(".steam").join("steam").join("steamapps").join("common"));
            roots.push(h.join(".var").join("app")); // Flatpak
        }
    }

    #[cfg(target_os = "windows")]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(v) = std::env::var(var) {
                let p = PathBuf::from(v);
                roots.push(p.clone());
                // Steam's canonical location under Program Files (x86).
                if var == "ProgramFiles(x86)" {
                    roots.push(p.join("Steam").join("steamapps").join("common"));
                }
            }
        }
        if let Some(h) = &home {
            roots.push(h.join("Games"));
            roots.push(h.join("Documents").join("Games"));
        }
        // Steam install path from the registry — covers users who installed
        // Steam somewhere other than Program Files (x86), which is common
        // on systems with small C: drives.
        if let Some(steam) = steam_install_path_windows() {
            roots.push(steam.join("steamapps").join("common"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications"));
        if let Some(h) = &home {
            roots.push(h.join("Applications"));
            roots.push(
                h.join("Library")
                    .join("Application Support")
                    .join("Steam")
                    .join("steamapps")
                    .join("common"),
            );
        }
    }

    roots
}

#[cfg(target_os = "windows")]
fn steam_install_path_windows() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    // Both HKCU and HKLM carry the install path depending on whether Steam
    // was installed per-user or system-wide. Try HKCU first (more likely
    // on modern Windows).
    for (root, subkey) in [
        (HKEY_CURRENT_USER, "Software\\Valve\\Steam"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Valve\\Steam"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\Valve\\Steam"),
    ] {
        if let Ok(key) = RegKey::predef(root).open_subkey(subkey) {
            if let Ok(path) = key.get_value::<String, _>("InstallPath") {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}
