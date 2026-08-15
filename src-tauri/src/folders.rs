//! Which subfolders of the demos folder get backed up, and which get listed.
//!
//! People organise their demos. A folder per map, a folder per year, an `old/`
//! nobody wants to look at again, a `friends/` full of other people's runs.
//! Watching subfolders is one switch today: either every one of them is backed
//! up and listed, or none is. That is the wrong shape for a folder of somebody
//! else's demos, which should not be uploaded under your account, and for an
//! archive of a thousand old runs, which should not be in the way of the twelve
//! from tonight.
//!
//! So each subfolder has two answers: **sync** (back its demos up) and
//! **visible** (show them in the list). They are separate because the useful
//! combinations are not the same question - an archive stays backed up and out
//! of sight, a friend's folder stays visible and unsent.
//!
//! ## Only the exceptions are stored
//!
//! The config keeps a record for a folder **only once its answer differs from
//! what it would inherit anyway**. Everything else is synced and visible,
//! including folders that do not exist yet.
//!
//! The alternative - writing down every subfolder with both answers - looks
//! tidier and is wrong: a folder created next month would have no record, and
//! the code would have to guess whether that means "new, so include it" or
//! "deliberately absent, so do not". Storing exceptions makes the default
//! automatic and the config small. It also means the file does not rot when a
//! folder is renamed or deleted; a stale record simply never matches anything.
//!
//! ## Deeper folders inherit
//!
//! Turning off `2019` turns off `2019/january` too, because that is what
//! anybody means by it. A record on the deeper folder wins over the shallower
//! one, so `old` off with `old/keep-these` on is expressible - the nearest
//! ancestor holding a record decides, and if none does, the default applies.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn default_true() -> bool {
    true
}

/// A subfolder of the demos folder whose answer differs from its parent's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedFolder {
    /// Relative to the demos folder, `/`-separated, no leading or trailing
    /// slash. Relative rather than absolute so moving or renaming the demos
    /// folder keeps every rule intact.
    pub path: String,
    /// Back demos in this folder up to defrag.racing.
    #[serde(default = "default_true")]
    pub sync: bool,
    /// Show demos in this folder in the Demos list.
    #[serde(default = "default_true")]
    pub visible: bool,
}

/// Normalise a relative folder path into the form rules are compared in:
/// forward slashes, no empty segments, lowercased.
///
/// Lowercased because Windows paths are case-insensitive and the same folder
/// reaches us spelled both ways - typed into a rule as `Old`, walked off disk
/// as `old`. Linux is case-sensitive and this makes two genuinely different
/// folders share a rule there; folders differing only in case are vanishingly
/// rare, and the failure is one folder inheriting the other's setting rather
/// than anything being lost.
pub fn key(rel: &str) -> String {
    rel.split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase()
}

/// How deep a folder sits below the demos folder. 1 for a direct child.
pub fn depth(rel: &str) -> usize {
    let k = key(rel);
    if k.is_empty() {
        0
    } else {
        k.matches('/').count() + 1
    }
}

/// What `rel` would resolve to from the records ABOVE it, ignoring any record
/// on `rel` itself. This is what a record has to differ from to be worth
/// keeping.
pub fn inherited(folders: &[WatchedFolder], rel: &str) -> (bool, bool) {
    let k = key(rel);
    let mut cur = match k.rfind('/') {
        Some(i) => k[..i].to_string(),
        None => String::new(),
    };
    loop {
        if cur.is_empty() {
            return (true, true); // the demos folder itself is always both
        }
        if let Some(f) = folders.iter().find(|f| key(&f.path) == cur) {
            return (f.sync, f.visible);
        }
        match cur.rfind('/') {
            Some(i) => cur.truncate(i),
            None => return (true, true),
        }
    }
}

/// `(sync, visible, has_a_record_of_its_own)` for a folder.
pub fn effective(folders: &[WatchedFolder], rel: &str) -> (bool, bool, bool) {
    let k = key(rel);
    if !k.is_empty() {
        if let Some(f) = folders.iter().find(|x| key(&x.path) == k) {
            return (f.sync, f.visible, true);
        }
    }
    let (s, v) = inherited(folders, rel);
    (s, v, false)
}

/// Record the user's answer for one folder, then drop every record that says
/// nothing its parent does not already say.
///
/// The pruning is why this is a function rather than a push: turning a whole
/// tree off would otherwise leave a record per folder, and turning the parent
/// back on later would leave every child stuck off with no visible cause.
pub fn apply(folders: &mut Vec<WatchedFolder>, rel: &str, sync: bool, visible: bool) {
    let k = key(rel);
    if k.is_empty() {
        return; // the demos folder itself is not a rule
    }
    folders.retain(|f| key(&f.path) != k);
    folders.push(WatchedFolder { path: k, sync, visible });
    prune(folders);
}

fn prune(folders: &mut Vec<WatchedFolder>) {
    // Shallowest first, so a parent is decided before the children that
    // inherit from it.
    folders.sort_by_key(|f| (depth(&f.path), key(&f.path)));
    let mut kept: Vec<WatchedFolder> = Vec::new();
    for f in folders.iter() {
        if (f.sync, f.visible) != inherited(&kept, &f.path) {
            kept.push(f.clone());
        }
    }
    *folders = kept;
}

/// The folder of `file` relative to `root`, as a rule key. `None` when the file
/// is not under the root at all.
///
/// Both sides are normalised first: on Windows the watcher reports
/// `\\?\E:\...` while a directory walk reports `E:\...`, and a plain
/// `strip_prefix` between the two forms fails - which would silently drop every
/// rule. See `cache::normalize`.
pub fn key_for_file(root: &Path, file: &Path) -> Option<String> {
    let root = crate::cache::normalize(root);
    let file = crate::cache::normalize(file);
    let parent = file.parent()?;
    relative_to(&root, parent).map(|rel| key(&rel))
}

/// `child` relative to `base`, as a string, or `None` if it is not below it.
/// Case-insensitive on Windows, where two spellings of a drive letter are the
/// same folder.
pub fn relative_to(base: &Path, child: &Path) -> Option<String> {
    if let Ok(rel) = child.strip_prefix(base) {
        return Some(rel.to_string_lossy().to_string());
    }
    #[cfg(windows)]
    {
        let b = base.to_string_lossy().to_lowercase();
        let c = child.to_string_lossy().to_lowercase();
        let rest = c.strip_prefix(&b)?;
        // Length-matched against the original so the caller gets the real
        // spelling back, not the lowercased one.
        let cut = child.to_string_lossy().len() - rest.len();
        return Some(
            child.to_string_lossy()[cut..]
                .trim_start_matches(['\\', '/'])
                .to_string(),
        );
    }
    #[cfg(not(windows))]
    None
}

#[derive(Default, Clone)]
struct Rules {
    root: Option<PathBuf>,
    /// Only meaningful when subfolders are watched at all; with that switch off
    /// there are no subfolders in play and every rule is moot.
    subfolders: bool,
    folders: Vec<WatchedFolder>,
}

/// Shared, cheap to read, refreshed whenever the config is saved.
///
/// Lives in `AppState` rather than being handed to the watcher at start, so a
/// folder switched off mid-session takes effect on the next demo instead of on
/// the next Stop+Start.
#[derive(Default)]
pub struct FolderState {
    inner: Mutex<Rules>,
}

impl FolderState {
    /// Load straight from the persisted config, so there is one way this gets
    /// filled. Called at boot and after every config write.
    pub fn reload(&self) {
        if let Ok(cfg) = crate::config::Config::load() {
            *self.inner.lock().unwrap() = Rules {
                root: cfg.demos_path.clone(),
                subfolders: cfg.include_subfolders,
                folders: cfg.folders.clone(),
            };
        }
    }

    /// Should this demo be backed up?
    pub fn allows_sync(&self, file: &Path) -> bool {
        self.decide(file, 0)
    }

    /// Should this demo appear in the Demos list?
    pub fn allows_listing(&self, file: &Path) -> bool {
        self.decide(file, 1)
    }

    fn decide(&self, file: &Path, which: u8) -> bool {
        let rules = self.inner.lock().unwrap();
        if !rules.subfolders || rules.folders.is_empty() {
            return true;
        }
        let Some(root) = rules.root.as_ref() else { return true };
        match key_for_file(root, file) {
            Some(k) => {
                let (sync, visible, _) = effective(&rules.folders, &k);
                if which == 0 { sync } else { visible }
            }
            // A demo outside the watched folder entirely - a staged copy, a
            // file the user pointed at by hand. Rules about subfolders have
            // nothing to say about it.
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(entries: &[(&str, bool, bool)]) -> Vec<WatchedFolder> {
        entries
            .iter()
            .map(|(p, sync, visible)| WatchedFolder {
                path: p.to_string(),
                sync: *sync,
                visible: *visible,
            })
            .collect()
    }

    #[test]
    fn a_folder_with_no_record_is_synced_and_visible() {
        let f = set(&[("old", false, false)]);
        assert_eq!(effective(&f, "tonight"), (true, true, false));
        assert_eq!(effective(&f, ""), (true, true, false));
    }

    #[test]
    fn deeper_folders_inherit_the_nearest_record() {
        let f = set(&[("old", false, false), ("old/keep", true, true)]);
        assert_eq!(effective(&f, "old"), (false, false, true));
        assert_eq!(effective(&f, "old/2019/january"), (false, false, false));
        assert_eq!(effective(&f, "old/keep"), (true, true, true));
        assert_eq!(effective(&f, "old/keep/deeper"), (true, true, false));
    }

    #[test]
    fn sync_and_visible_are_answered_separately() {
        let f = set(&[("archive", true, false), ("friends", false, true)]);
        assert_eq!(effective(&f, "archive"), (true, false, true));
        assert_eq!(effective(&f, "friends"), (false, true, true));
    }

    #[test]
    fn separators_and_case_do_not_change_a_rule() {
        assert_eq!(key("Old\\2019\\"), "old/2019");
        assert_eq!(key("/old//2019"), "old/2019");
        let f = set(&[("Old\\2019", false, true)]);
        assert_eq!(effective(&f, "old/2019"), (false, true, true));
    }

    #[test]
    fn a_record_that_matches_its_parent_is_not_kept() {
        let mut f = set(&[("old", false, false)]);
        apply(&mut f, "old/2019", false, false);
        assert_eq!(f.len(), 1, "the child says nothing new");
        assert_eq!(effective(&f, "old/2019"), (false, false, false));
    }

    #[test]
    fn a_child_can_be_turned_back_on_under_a_parent_that_is_off() {
        let mut f = set(&[("old", false, false)]);
        apply(&mut f, "old/keep", true, true);
        assert_eq!(effective(&f, "old/keep"), (true, true, true));
        assert_eq!(effective(&f, "old/other"), (false, false, false));
    }

    #[test]
    fn turning_a_parent_back_on_does_not_strand_its_children() {
        let mut f = Vec::new();
        apply(&mut f, "old", false, false);
        apply(&mut f, "old/2019", false, false); // redundant, dropped
        apply(&mut f, "old", true, true);
        assert!(f.is_empty(), "nothing differs from the default any more");
        assert_eq!(effective(&f, "old/2019"), (true, true, false));
    }

    #[test]
    fn a_file_in_the_root_folder_is_never_ruled_out() {
        let f = set(&[("old", false, false)]);
        let k = key_for_file(Path::new("/demos"), Path::new("/demos/run.dm_68")).unwrap();
        assert_eq!(k, "");
        assert_eq!(effective(&f, &k), (true, true, false));
    }

    #[test]
    fn a_file_below_a_ruled_folder_picks_that_rule_up() {
        let k = key_for_file(Path::new("/demos"), Path::new("/demos/old/2019/run.dm_68")).unwrap();
        assert_eq!(k, "old/2019");
    }
}
