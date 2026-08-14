//! Opening a `.dm_68` from the file manager.
//!
//! Two separate things, deliberately kept apart, because they carry completely
//! different weight.
//!
//! **The right-click entry** - "Play in Defrag Launcher" - hangs off the file
//! EXTENSION, not off any program. It appears next to whatever the user already
//! uses, changes no default, and takes nothing away: DemoCleaner3 stays exactly
//! where it was. It is registered on install and re-registered on every start,
//! because it costs nothing and repairs itself after a move or a reinstall.
//!
//! **Being the default program** is the user's decision and is only ever made
//! by them, once, from inside the app. Windows guards defaults with a
//! `UserChoice` key that applications are not supposed to write - so when one
//! exists and points elsewhere, this reports the truth rather than pretending
//! it won: the caller is told who owns the type so it can send the person to
//! the Open-with dialog instead of silently doing nothing.
//!
//! Everything lives under HKEY_CURRENT_USER. The launcher installs per user
//! (`installMode: currentUser`), so a machine-wide write would fail on a
//! standard account and would be wrong even where it worked.

use serde::Serialize;

/// What the OS currently thinks about `.dm_68`.
#[derive(Debug, Clone, Serialize)]
pub struct AssocStatus {
    /// False on platforms where none of this applies - the UI hides the whole
    /// section rather than offering a button that cannot work.
    pub supported: bool,
    /// Is the right-click entry there?
    pub context_menu: bool,
    /// Are we the program that opens a double-clicked demo?
    pub is_default: bool,
    /// Who owns the type, when it is not us. A raw ProgID (`DemoCleaner3.dm_68`)
    /// - useful in a log and in a support question, not meant for display.
    pub default_owner: Option<String>,
}

impl AssocStatus {
    fn unsupported() -> Self {
        Self { supported: false, context_menu: false, is_default: false, default_owner: None }
    }
}

#[cfg(windows)]
mod imp {
    use super::AssocStatus;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    use winreg::RegKey;

    /// Our file type. Windows keys associations by ProgID, not by executable.
    pub const PROGID: &str = "DefragRacingLauncher.Demo";

    const EXT: &str = ".dm_68";

    /// The verb key name. Not shown to anyone - the label is the key's default
    /// value - but it must be stable, or an update leaves a second entry in
    /// everybody's context menu.
    const VERB: &str = "PlayInDefragLauncher";

    fn exe_path() -> Result<String, String> {
        std::env::current_exe()
            .map_err(|e| format!("Could not find the launcher's own path: {e}"))
            .map(|p| p.to_string_lossy().to_string())
    }

    /// Register the ProgID and the right-click entry. Idempotent, and safe to
    /// call on every start: it writes the same values over themselves.
    pub fn register() -> Result<(), String> {
        let exe = exe_path()?;
        let classes = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
            .map_err(|e| format!("Could not open the registry: {e}"))?;

        let write = |e: std::io::Error| format!("Could not write to the registry: {e}");

        // The ProgID: what "Open with" lists us as, and what an association
        // points at if the user ever makes us the default.
        let (progid, _) = classes.create_subkey(PROGID).map_err(write)?;
        progid.set_value("", &"Quake 3 Defrag demo").map_err(write)?;

        let (icon, _) = progid.create_subkey("DefaultIcon").map_err(write)?;
        icon.set_value("", &format!("\"{exe}\",0")).map_err(write)?;

        let (open_cmd, _) = progid.create_subkey("shell\\open\\command").map_err(write)?;
        open_cmd.set_value("", &format!("\"{exe}\" \"%1\"")).map_err(write)?;

        // The right-click entry, hung off the extension rather than off a
        // ProgID, so it is there whatever program owns the file type.
        let (verb, _) = classes
            .create_subkey(format!("SystemFileAssociations\\{EXT}\\shell\\{VERB}"))
            .map_err(write)?;
        verb.set_value("", &"Play in Defrag Launcher").map_err(write)?;
        verb.set_value("Icon", &format!("\"{exe}\",0")).map_err(write)?;

        let (verb_cmd, _) = verb.create_subkey("command").map_err(write)?;
        verb_cmd.set_value("", &format!("\"{exe}\" \"%1\"")).map_err(write)?;

        // Offer ourselves in the Open-with list without claiming the type.
        let (open_with, _) = classes
            .create_subkey(format!("{EXT}\\OpenWithProgids"))
            .map_err(write)?;
        open_with.set_value(PROGID, &"").map_err(write)?;

        Ok(())
    }

    pub fn status() -> AssocStatus {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        let context_menu = hkcu
            .open_subkey(format!("Software\\Classes\\SystemFileAssociations\\{EXT}\\shell\\{VERB}\\command"))
            .is_ok();

        // UserChoice is what Explorer actually honours, and it is written by
        // Windows itself when somebody picks a program. It outranks the plain
        // class association, so reading only the latter would report us as the
        // default while double-clicking still opened DemoCleaner3.
        let user_choice: Option<String> = hkcu
            .open_subkey(format!(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\{EXT}\\UserChoice"
            ))
            .ok()
            .and_then(|k| k.get_value("ProgId").ok());

        let class_default: Option<String> = hkcu
            .open_subkey(format!("Software\\Classes\\{EXT}"))
            .ok()
            .and_then(|k| k.get_value("").ok());

        let owner = user_choice.or(class_default).filter(|v| !v.is_empty());

        AssocStatus {
            supported: true,
            context_menu,
            is_default: owner.as_deref() == Some(PROGID),
            default_owner: owner,
        }
    }

    /// Claim the file type, as far as an application is allowed to.
    ///
    /// Returns the status afterwards, so the caller can see whether it took.
    /// It does not when Windows already holds a UserChoice for somebody else -
    /// that key is signed and only the OS may write it, and forging it is
    /// exactly the behaviour that gets installers flagged as malware. In that
    /// case the honest move is to say so and let the person pick us in the
    /// Open-with dialog, which writes UserChoice properly.
    pub fn make_default() -> Result<AssocStatus, String> {
        register()?;

        let classes = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
            .map_err(|e| format!("Could not open the registry: {e}"))?;

        let (ext, _) = classes
            .create_subkey(EXT)
            .map_err(|e| format!("Could not write to the registry: {e}"))?;
        ext.set_value("", &PROGID)
            .map_err(|e| format!("Could not write to the registry: {e}"))?;

        Ok(status())
    }

    /// Take the right-click entry and the ProgID back out. Used by the
    /// uninstaller's hook; the app itself never calls it.
    #[allow(dead_code)]
    pub fn unregister() -> Result<(), String> {
        let classes = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
            .map_err(|e| format!("Could not open the registry: {e}"))?;

        let _ = classes.delete_subkey_all(format!("SystemFileAssociations\\{EXT}\\shell\\{VERB}"));
        let _ = classes.delete_subkey_all(PROGID);

        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::AssocStatus;

    pub fn register() -> Result<(), String> {
        Ok(())
    }

    pub fn status() -> AssocStatus {
        AssocStatus::unsupported()
    }

    pub fn make_default() -> Result<AssocStatus, String> {
        Err("Setting the default program for a file type is Windows-only here.".into())
    }
}

/// Put the right-click entry in place. Called at startup, best effort: a
/// launcher that cannot write its own HKCU keys still works, it just does not
/// appear in the context menu.
pub fn register_quietly() {
    if let Err(e) = imp::register() {
        eprintln!("[file_assoc] {e}");
    }
}

pub fn status() -> AssocStatus {
    imp::status()
}

pub fn make_default() -> Result<AssocStatus, String> {
    imp::make_default()
}

/// Does this filename look like a Quake 3 demo? `.dm_68` is Defrag's, and the
/// older protocols are still played, so anything `.dm_6x` counts.
pub fn looks_like_a_demo(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e.starts_with("dm_") && e.len() <= 6
        })
        .unwrap_or(false)
}

/// The demo file in a command line, if there is one.
///
/// Every other argument is skipped rather than guessed at: switches, the
/// executable's own path, and a `defrag://` URL, which has its own handler and
/// must not be swallowed here.
pub fn demo_path_in_args<I, S>(args: I) -> Option<std::path::PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().skip(1).find_map(|arg| {
        let arg = arg.as_ref();

        if arg.starts_with('-') || arg.starts_with("defrag://") {
            return None;
        }

        let path = std::path::PathBuf::from(arg);

        (looks_like_a_demo(&path) && path.is_file()).then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognises_demo_extensions() {
        assert!(looks_like_a_demo(&PathBuf::from("run[df.cpm]01.234(nick).dm_68")));
        assert!(looks_like_a_demo(&PathBuf::from("old.dm_66")));
        assert!(!looks_like_a_demo(&PathBuf::from("notes.txt")));
        assert!(!looks_like_a_demo(&PathBuf::from("archive.dm_68.zip")));
    }

    #[test]
    fn skips_switches_and_deep_links() {
        // Nothing here is a file that exists, so the result is None either way;
        // what matters is that neither the flag nor the URL is even considered
        // a candidate.
        assert_eq!(demo_path_in_args(["launcher.exe", "--hidden"]), None);
        assert_eq!(demo_path_in_args(["launcher.exe", "defrag://connect/1.2.3.4"]), None);
    }

    #[test]
    fn finds_a_real_file() {
        let dir = std::env::temp_dir().join("defrag-launcher-assoc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let demo = dir.join("stage[df.cpm]01.234(nick).dm_68");
        std::fs::write(&demo, b"not really a demo").unwrap();

        let found = demo_path_in_args(["launcher.exe", demo.to_str().unwrap()]);
        assert_eq!(found.as_deref(), Some(demo.as_path()));

        std::fs::remove_file(&demo).ok();
    }
}
