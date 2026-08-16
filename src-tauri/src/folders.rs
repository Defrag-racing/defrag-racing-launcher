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
//! what it would inherit anyway**. Everything else follows the folder it sits
//! in, and the folders at the top follow the watched folder's own default -
//! including folders that do not exist yet.
//!
//! The alternative - writing down every subfolder with both answers - looks
//! tidier and is wrong: a folder created next month would have no record, and
//! the code would have to guess whether that means "new, so include it" or
//! "deliberately absent, so do not". Storing exceptions makes the default
//! automatic and the config small. It also means the file does not rot when a
//! folder is renamed or deleted; a stale record simply never matches anything.
//!
//! ## Subfolders start off
//!
//! A new watched folder backs up and shows what is directly in it, and nothing
//! deeper, until somebody ticks a subfolder. Pointing the launcher at a folder
//! is not consent to publish the archive underneath it - an `old/` with eight
//! thousand runs in it is somebody's history, not tonight's session - so the
//! answer for anything below the top is off until it is asked for.
//!
//! That default is per watched folder (`sub_sync` / `sub_visible`), which is
//! also how the old single "include subfolders" switch survives: a config that
//! had it on keeps every subfolder on, because for that person they always
//! were.
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

/// A folder full of demos the launcher watches.
///
/// The first one is the game's own `demos` folder, the one Defrag writes into,
/// and it is the one Settings has always had. People keep demos elsewhere too -
/// a second drive, an archive of somebody else's runs, a collection that
/// predates the launcher - and those are added here. Each carries its own two
/// answers and its own rules for the folders inside it, because the reason for
/// adding a whole drive of somebody else's demos is usually "show me these, do
/// not put them on my account".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoRoot {
    pub path: PathBuf,
    /// Back its demos up to defrag.racing.
    #[serde(default = "default_true")]
    pub sync: bool,
    /// Show its demos in the Demos list.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// What a subfolder with no record of its own does. Off, so that adding a
    /// folder never quietly publishes an archive nobody asked about, and so a
    /// folder made next month is off too rather than the code having to guess.
    #[serde(default)]
    pub sub_sync: bool,
    #[serde(default)]
    pub sub_visible: bool,
    /// Exceptions for the folders inside it, exactly as for the main one.
    #[serde(default)]
    pub folders: Vec<WatchedFolder>,
}

/// Which root a file belongs to: the DEEPEST one that contains it.
///
/// Depth matters because a root inside another root is refused when it is
/// added, but a config written by hand, or an older one, can still hold a pair
/// like that - and then the nearer folder is the one whose answers the user
/// was thinking about.
pub fn root_for<'a>(roots: &'a [DemoRoot], file: &Path) -> Option<&'a DemoRoot> {
    roots
        .iter()
        .filter(|r| key_for_file(&r.path, file).is_some())
        .max_by_key(|r| crate::cache::normalize(&r.path).as_os_str().len())
}

/// Is `candidate` the same folder as `other`, inside it, or its parent?
///
/// Two roots that overlap would ask the same demo two questions and answer it
/// twice, so adding one is refused.
pub fn overlaps(candidate: &Path, other: &Path) -> bool {
    let a = crate::cache::normalize(candidate);
    let b = crate::cache::normalize(other);
    relative_to(&a, &b).is_some() || relative_to(&b, &a).is_some()
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
///
/// `sub` is the watched folder's own default, which is what a folder at the
/// top inherits when no record above it says otherwise.
pub fn inherited(folders: &[WatchedFolder], rel: &str, sub: (bool, bool)) -> (bool, bool) {
    let k = key(rel);
    let mut cur = match k.rfind('/') {
        Some(i) => k[..i].to_string(),
        None => String::new(),
    };
    loop {
        if cur.is_empty() {
            return sub;
        }
        if let Some(f) = folders.iter().find(|f| key(&f.path) == cur) {
            return (f.sync, f.visible);
        }
        match cur.rfind('/') {
            Some(i) => cur.truncate(i),
            None => return sub,
        }
    }
}

/// `(sync, visible, has_a_record_of_its_own)` for a folder.
///
/// The watched folder itself (an empty `rel`) is not a subfolder and has no
/// default to fall back on - its own two answers are the caller's, and it asks
/// about them directly.
pub fn effective(folders: &[WatchedFolder], rel: &str, sub: (bool, bool)) -> (bool, bool, bool) {
    let k = key(rel);
    if !k.is_empty() {
        if let Some(f) = folders.iter().find(|x| key(&x.path) == k) {
            return (f.sync, f.visible, true);
        }
    }
    let (s, v) = inherited(folders, rel, sub);
    (s, v, false)
}

/// Record the user's answer for one folder, then drop every record that says
/// nothing its parent does not already say.
///
/// The pruning is why this is a function rather than a push: turning a whole
/// tree off would otherwise leave a record per folder, and turning the parent
/// back on later would leave every child stuck off with no visible cause.
pub fn apply(
    folders: &mut Vec<WatchedFolder>,
    rel: &str,
    sync: bool,
    visible: bool,
    sub: (bool, bool),
) {
    let k = key(rel);
    if k.is_empty() {
        return; // the demos folder itself is not a rule
    }
    folders.retain(|f| key(&f.path) != k);
    folders.push(WatchedFolder { path: k, sync, visible });
    prune(folders, sub);
}

/// Drop every record that says nothing its parent does not already say. Also
/// what makes a changed default tidy up after itself: switch a whole folder's
/// subfolders on and the records that were saying "on" one at a time go.
pub fn prune(folders: &mut Vec<WatchedFolder>, sub: (bool, bool)) {
    // Shallowest first, so a parent is decided before the children that
    // inherit from it.
    folders.sort_by_key(|f| (depth(&f.path), key(&f.path)));
    let mut kept: Vec<WatchedFolder> = Vec::new();
    for f in folders.iter() {
        if (f.sync, f.visible) != inherited(&kept, &f.path, sub) {
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
    /// Every watched folder, the game's own first.
    roots: Vec<DemoRoot>,
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
                roots: cfg.demo_roots(),
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

        // A demo outside every watched folder - a staged copy, a file the user
        // pointed at by hand. None of these rules are about it.
        let Some(root) = root_for(&rules.roots, file) else { return true };

        // The folder's own answer comes first: a whole drive switched off is
        // off, whatever the folders inside it say.
        let root_says = if which == 0 { root.sync } else { root.visible };
        if !root_says {
            return false;
        }

        match key_for_file(&root.path, file) {
            // Sitting directly in the watched folder, which has already said
            // yes. Subfolder defaults are not about this file.
            Some(k) if k.is_empty() => true,
            Some(k) => {
                let (sync, visible, _) =
                    effective(&root.folders, &k, (root.sub_sync, root.sub_visible));
                if which == 0 { sync } else { visible }
            }
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

    /// Every subfolder on, which is what a config that had the old "include
    /// subfolders" switch turned on carries forward.
    const ON: (bool, bool) = (true, true);
    /// The default for anything new: what is in the folder itself, and nothing
    /// deeper until it is asked for.
    const OFF: (bool, bool) = (false, false);

    #[test]
    fn a_folder_with_no_record_follows_the_watched_folders_default() {
        let f = set(&[("old", false, false)]);
        assert_eq!(effective(&f, "tonight", ON), (true, true, false));
        assert_eq!(effective(&f, "tonight", OFF), (false, false, false));
    }

    #[test]
    fn a_subfolder_ticked_on_is_on_though_the_rest_stay_off() {
        let mut f = Vec::new();
        apply(&mut f, "tonight", true, true, OFF);
        assert_eq!(effective(&f, "tonight", OFF), (true, true, true));
        assert_eq!(effective(&f, "tonight/deeper", OFF), (true, true, false));
        assert_eq!(effective(&f, "old", OFF), (false, false, false));
    }

    #[test]
    fn deeper_folders_inherit_the_nearest_record() {
        let f = set(&[("old", false, false), ("old/keep", true, true)]);
        assert_eq!(effective(&f, "old", ON), (false, false, true));
        assert_eq!(effective(&f, "old/2019/january", ON), (false, false, false));
        assert_eq!(effective(&f, "old/keep", ON), (true, true, true));
        assert_eq!(effective(&f, "old/keep/deeper", ON), (true, true, false));
    }

    #[test]
    fn sync_and_visible_are_answered_separately() {
        let f = set(&[("archive", true, false), ("friends", false, true)]);
        assert_eq!(effective(&f, "archive", ON), (true, false, true));
        assert_eq!(effective(&f, "friends", ON), (false, true, true));
    }

    #[test]
    fn separators_and_case_do_not_change_a_rule() {
        assert_eq!(key("Old\\2019\\"), "old/2019");
        assert_eq!(key("/old//2019"), "old/2019");
        let f = set(&[("Old\\2019", false, true)]);
        assert_eq!(effective(&f, "old/2019", ON), (false, true, true));
    }

    #[test]
    fn a_record_that_matches_its_parent_is_not_kept() {
        let mut f = set(&[("old", false, false)]);
        apply(&mut f, "old/2019", false, false, ON);
        assert_eq!(f.len(), 1, "the child says nothing new");
        assert_eq!(effective(&f, "old/2019", ON), (false, false, false));
    }

    #[test]
    fn a_child_can_be_turned_back_on_under_a_parent_that_is_off() {
        let mut f = set(&[("old", false, false)]);
        apply(&mut f, "old/keep", true, true, ON);
        assert_eq!(effective(&f, "old/keep", ON), (true, true, true));
        assert_eq!(effective(&f, "old/other", ON), (false, false, false));
    }

    #[test]
    fn turning_a_parent_back_on_does_not_strand_its_children() {
        let mut f = Vec::new();
        apply(&mut f, "old", false, false, ON);
        apply(&mut f, "old/2019", false, false, ON); // redundant, dropped
        apply(&mut f, "old", true, true, ON);
        assert!(f.is_empty(), "nothing differs from the default any more");
        assert_eq!(effective(&f, "old/2019", ON), (true, true, false));
    }

    #[test]
    fn switching_the_default_on_clears_the_records_that_said_so_one_by_one() {
        let mut f = Vec::new();
        apply(&mut f, "tonight", true, true, OFF);
        apply(&mut f, "check", true, true, OFF);
        assert_eq!(f.len(), 2);
        prune(&mut f, ON);
        assert!(f.is_empty(), "they all say what the default now says");
    }

    #[test]
    fn a_file_in_the_root_folder_is_never_ruled_out() {
        let f = set(&[("old", false, false)]);
        let k = key_for_file(Path::new("/demos"), Path::new("/demos/run.dm_68")).unwrap();
        assert_eq!(k, "");
        assert_eq!(effective(&f, &k, ON), (true, true, false));
    }

    /// The watched folder's own demos are its own business: a default of "not
    /// the subfolders" must not be read as "not this folder either".
    #[test]
    fn the_watched_folders_own_demos_survive_a_default_of_off() {
        let state = FolderState::default();
        *state.inner.lock().unwrap() = Rules {
            roots: vec![DemoRoot {
                path: PathBuf::from("/demos"),
                sync: true,
                visible: true,
                sub_sync: false,
                sub_visible: false,
                folders: Vec::new(),
            }],
        };
        assert!(state.allows_sync(Path::new("/demos/run.dm_68")));
        assert!(!state.allows_sync(Path::new("/demos/old/run.dm_68")));
        assert!(state.allows_listing(Path::new("/demos/run.dm_68")));
        assert!(!state.allows_listing(Path::new("/demos/old/run.dm_68")));
    }

    #[test]
    fn a_file_below_a_ruled_folder_picks_that_rule_up() {
        let k = key_for_file(Path::new("/demos"), Path::new("/demos/old/2019/run.dm_68")).unwrap();
        assert_eq!(k, "old/2019");
    }
}
