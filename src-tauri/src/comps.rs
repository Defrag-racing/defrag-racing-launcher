//! Comps - the weekly competition - as far as the launcher is concerned.
//!
//! Two jobs here. The visible one feeds the Comps tab: which map each physics
//! is playing, when the round ends, what the user's own entries are doing.
//!
//! The load-bearing one is the **guard**. Today every new demo in the watched
//! folder goes to `/api/launcher/upload-demo`, which publishes it the moment it
//! lands. A run on this week's comps map taking that route would publish the
//! user's own time - and their route - in the middle of the round, and that
//! cannot be taken back. So a demo whose filename says it is a run of a comps
//! map never travels the ordinary path: it is either entered into comps or it
//! waits for the user to say which of the two it is.
//!
//! The map is read from the filename (`mapname[df.cpm]time.dm_68`), which is a
//! convention rather than a promise. That is deliberate and the server knows
//! it: an entry the launcher decided on carries `auto: true`, and if parsing
//! the demo shows it is not a run of that map at all, the server withdraws the
//! entry and leaves an ordinary upload behind. Guessing wrong therefore costs a
//! round trip, while not guessing at all costs a published run.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a fetched payload is treated as current. The round itself changes
/// once a week; this interval exists so a round that ENDS while the launcher
/// is open stops holding demos within a few minutes.
pub const REFRESH_SECS: u64 = 300;

/// How long a snapshot is trusted when its round carries no `ends_at` at all.
/// That should not happen - the scheduler always sets one - but a payload we
/// cannot date must not hold demos forever, and must not wave them through
/// either. One hour is long enough to survive a network outage mid-round and
/// short enough that a stale file on disk cannot hold anything tomorrow.
const NO_DEADLINE_TRUST_SECS: u64 = 3600;

/// What the launcher does with a demo that looks like a run of a comps map.
///
/// The default is `Ask` because the two mistakes are not the same size. The
/// worst case of holding a demo is one extra click. The worst case of not
/// holding it is somebody's run published mid-round, which no button undoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompsMode {
    /// Hold the file and let the user choose: enter it, or upload it normally.
    Ask,
    /// Enter it into the round without asking.
    Auto,
    /// No guard at all - every demo takes the ordinary path.
    Off,
}

impl Default for CompsMode {
    fn default() -> Self {
        Self::Ask
    }
}

impl CompsMode {
    fn as_u8(self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::Auto => 1,
            Self::Off => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Auto,
            2 => Self::Off,
            _ => Self::Ask,
        }
    }
}

/// The last payload `/api/launcher/comps` gave us.
///
/// `raw` is forwarded to the frontend untouched, the same way the server
/// browser works: the shape belongs to the Laravel side and will grow columns,
/// and a launcher release should not be the price of adding one. `guard` is the
/// handful of fields the watcher needs, extracted once so the hot path never
/// walks a JSON tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompsSnapshot {
    pub raw: serde_json::Value,
    pub guard: Option<GuardRound>,
    pub fetched_at_ms: u64,
}

/// The round being played, reduced to what the guard decides on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardRound {
    pub round_id: i64,
    pub comp_number: i64,
    /// Epoch ms, parsed from the payload's ISO 8601 string. Times from the API
    /// carry their offset and are compared as instants - never formatted and
    /// re-parsed as local time, which is how a round once moved by two hours.
    pub ends_at_ms: Option<i64>,
    /// (physics, map name) with both sides lowercased, because map names on
    /// the site carry capitals on a few hundred maps and a filename never does.
    pub maps: Vec<(String, String)>,
}

impl GuardRound {
    fn from_payload(raw: &serde_json::Value) -> Option<Self> {
        let playing = raw.get("playing")?;
        if playing.is_null() {
            return None;
        }
        let round_id = playing.get("round_id")?.as_i64()?;
        let comp_number = playing.get("comp_number").and_then(|v| v.as_i64()).unwrap_or(0);
        let ends_at_ms = playing
            .get("ends_at")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_ms);

        let maps: Vec<(String, String)> = playing
            .get("maps")
            .and_then(|m| m.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(physics, name)| {
                        let name = name.as_str()?.trim();
                        if name.is_empty() {
                            return None;
                        }
                        Some((physics.to_ascii_lowercase(), name.to_ascii_lowercase()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if maps.is_empty() {
            return None;
        }

        Some(Self { round_id, comp_number, ends_at_ms, maps })
    }

    /// Is the round still taking runs, as of `now_ms`?
    ///
    /// `fetched_at_ms` only matters for the malformed case where the payload
    /// carried no deadline - see NO_DEADLINE_TRUST_SECS.
    fn is_open(&self, now_ms: u64, fetched_at_ms: u64) -> bool {
        match self.ends_at_ms {
            Some(ends) => ends > now_ms as i64,
            None => now_ms.saturating_sub(fetched_at_ms) < NO_DEADLINE_TRUST_SECS * 1000,
        }
    }
}

/// A filename the guard recognised as a run of a comps map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompsMatch {
    pub round_id: i64,
    pub comp_number: i64,
    /// Which physics' map it matched. Informational: a demo is entered into
    /// the round, and the server reads the real physics out of the file.
    pub physics: String,
    pub map: String,
}

/// Shared comps state: the cached payload plus the live guard mode.
///
/// The mode lives here rather than being read from config per file because the
/// watcher touches it once per demo and Settings has to be able to change it
/// without a restart. config.json stays the persisted truth; this is the copy
/// the hot path reads.
pub struct CompsState {
    inner: Mutex<Option<CompsSnapshot>>,
    mode: AtomicU8,
}

impl Default for CompsState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            mode: AtomicU8::new(CompsMode::default().as_u8()),
        }
    }
}

impl CompsState {
    pub fn mode(&self) -> CompsMode {
        CompsMode::from_u8(self.mode.load(Ordering::Acquire))
    }

    pub fn set_mode(&self, mode: CompsMode) {
        self.mode.store(mode.as_u8(), Ordering::Release);
    }

    pub fn snapshot(&self) -> Option<CompsSnapshot> {
        self.inner.lock().unwrap().clone()
    }

    /// True when the cached payload is younger than REFRESH_SECS - the Comps
    /// tab serves it without a round trip, so switching tabs is instant.
    pub fn is_fresh(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| now_ms().saturating_sub(s.fetched_at_ms) < REFRESH_SECS * 1000)
            .unwrap_or(false)
    }

    /// Adopt a freshly fetched payload and write it to disk.
    ///
    /// Persisting matters for the guard, not for the tab: a launcher that
    /// starts with no network mid-round would otherwise know nothing about the
    /// round and wave a run straight onto the site. With the file on disk it
    /// keeps holding until the round's own deadline passes.
    pub fn store(&self, raw: serde_json::Value) -> CompsSnapshot {
        let snapshot = self.adopt(raw);
        if let Err(e) = Self::save_to_disk(&snapshot) {
            log::warn!("comps: could not persist snapshot: {e}");
        }
        snapshot
    }

    /// Take a payload without writing it to disk.
    fn adopt(&self, raw: serde_json::Value) -> CompsSnapshot {
        let snapshot = CompsSnapshot {
            guard: GuardRound::from_payload(&raw),
            raw,
            fetched_at_ms: now_ms(),
        };
        *self.inner.lock().unwrap() = Some(snapshot.clone());
        snapshot
    }

    pub fn load_persisted(&self) {
        let Ok(path) = Self::path() else { return };
        let Ok(raw) = std::fs::read_to_string(&path) else { return };
        let Ok(snapshot) = serde_json::from_str::<CompsSnapshot>(&raw) else { return };
        *self.inner.lock().unwrap() = Some(snapshot);
    }

    pub fn clear_persisted() -> Result<()> {
        let path = Self::path()?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {:?}", path))?;
        }
        Ok(())
    }

    /// Does this filename look like a run of a map the open round is playing?
    ///
    /// Answers only from the cached round - deliberately no network call, so a
    /// user who is offline mid-round keeps the same protection they had while
    /// online. A launcher that has never seen a payload matches nothing and
    /// every demo takes the ordinary path; that is the one case where the guard
    /// cannot help, and pretending otherwise would mean holding every demo on
    /// a machine that simply has no token.
    pub fn guard_match(&self, filename: &str) -> Option<CompsMatch> {
        let map = map_name_from_filename(filename)?;
        let guard = self.inner.lock().unwrap();
        let snapshot = guard.as_ref()?;
        let round = snapshot.guard.as_ref()?;
        if !round.is_open(now_ms(), snapshot.fetched_at_ms) {
            return None;
        }
        // Map AND physics. A VQ3 run on the map being played in CPM cannot
        // enter the round and is not racing anybody in it, so it takes the
        // ordinary path and is published like any other demo. A filename that
        // does not say its physics is held anyway - that is the file we know
        // least about, on the map being competed on.
        let claimed = physics_from_filename(filename);
        let (physics, matched) = round.maps.iter().find(|(physics, name)| {
            *name == map && claimed.as_deref().map_or(true, |claimed| claimed == physics)
        })?;
        Some(CompsMatch {
            round_id: round.round_id,
            comp_number: round.comp_number,
            physics: physics.clone(),
            map: matched.clone(),
        })
    }

    /// The open round's id, whatever the filename says. Used when the user
    /// enters a demo by hand - there the choice is theirs, so the name does
    /// not have to agree with it.
    pub fn open_round_id(&self) -> Option<i64> {
        let guard = self.inner.lock().unwrap();
        let snapshot = guard.as_ref()?;
        let round = snapshot.guard.as_ref()?;
        round.is_open(now_ms(), snapshot.fetched_at_ms).then_some(round.round_id)
    }

    fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
            .context("could not resolve platform config directory")?;
        let dir = dirs.config_dir().to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
        Ok(dir.join("comps.json"))
    }

    fn save_to_disk(snapshot: &CompsSnapshot) -> Result<()> {
        let path = Self::path()?;
        let raw = serde_json::to_string(snapshot)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }
}

/// Map name out of a demo filename: everything before the first `[`.
///
/// Same cut the Demos view uses to link a row to its map page, so a demo the
/// user can click through to a map is a demo the guard can recognise. Returns
/// lowercase because the comparison on the other side is lowercase too.
pub fn map_name_from_filename(filename: &str) -> Option<String> {
    let idx = filename.find('[')?;
    if idx == 0 {
        return None;
    }
    let name = filename[..idx].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// Physics out of a demo filename: the word after the gametype inside the
/// brackets, so `map[df.cpm]12.345(nick).dm_68` says cpm. A fastcap adds a
/// third part (`[fc.cpm.3]`) which is not part of the physics.
///
/// A claim, like the map next to it - the server reads the real physics out of
/// the file afterwards and decides again.
pub fn physics_from_filename(filename: &str) -> Option<String> {
    let open = filename.find('[')?;
    let close = filename[open..].find(']')? + open;
    let mut parts = filename[open + 1..close].split('.');

    let _gametype = parts.next()?;
    let physics = parts.next()?.trim().to_ascii_lowercase();

    (physics == "cpm" || physics == "vq3").then_some(physics)
}

/// ISO 8601 with offset (what Carbon's toIso8601String writes) to epoch ms.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape /api/launcher/comps actually answers with, taken from a live
    /// response rather than from reading the PHP - only the map names and the
    /// deadline vary per case.
    fn payload(map_cpm: &str, ends_at: &str) -> serde_json::Value {
        json!({
            "playing": {
                "round_id": 7,
                "comp_number": 3,
                "category": "strafe",
                "weapon": null,
                "ends_at": ends_at,
                "prize_eur": 5,
                "maps": { "cpm": map_cpm, "vq3": "Fast-Cap-Two" },
                "entrants": { "cpm": 0, "vq3": 0 },
                "my_entries": []
            },
            "voting": null
        })
    }

    fn far_future() -> String {
        chrono::DateTime::from_timestamp_millis(now_ms() as i64 + 86_400_000)
            .unwrap()
            .to_rfc3339()
    }

    fn past() -> String {
        chrono::DateTime::from_timestamp_millis(now_ms() as i64 - 86_400_000)
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn map_comes_from_the_part_before_the_bracket() {
        assert_eq!(map_name_from_filename("cpm22[df.cpm]12.345(nick).dm_68").as_deref(), Some("cpm22"));
        // A demo the user renamed, or one from another game: no bracket, no
        // match - it takes the ordinary path and the guard says nothing.
        assert_eq!(map_name_from_filename("my run.dm_68"), None);
        assert_eq!(map_name_from_filename("[df.cpm].dm_68"), None);
    }

    #[test]
    fn matches_the_played_map_case_insensitively() {
        let state = CompsState::default();
        state.adopt(payload("Fast-Strafe", &far_future()));

        let hit = state.guard_match("fast-strafe[df.cpm]01.234(nick).dm_68").unwrap();
        assert_eq!(hit.round_id, 7);
        assert_eq!(hit.physics, "cpm");

        // The other physics' own map counts too, in its own physics.
        assert!(state.guard_match("fast-cap-two[df.vq3]09.000(nick).dm_68").is_some());
        assert!(state.guard_match("someothermap[df.cpm]01.234(nick).dm_68").is_none());

        // The right map in the WRONG physics is not this round's business: it
        // cannot enter and it is racing nobody, so it goes out like any other
        // demo.
        assert!(state.guard_match("fast-strafe[df.vq3]01.234(nick).dm_68").is_none());
        assert!(state.guard_match("fast-cap-two[df.cpm]09.000(nick).dm_68").is_none());

        // A filename that does not say its physics is held anyway.
        assert!(state.guard_match("fast-strafe[weird]01.234(nick).dm_68").is_some());
    }

    #[test]
    fn physics_comes_from_the_bracket() {
        assert_eq!(physics_from_filename("cpm22[df.cpm]12.345(nick).dm_68").as_deref(), Some("cpm"));
        assert_eq!(physics_from_filename("cpm22[mdf.vq3]12.345(nick).dm_68").as_deref(), Some("vq3"));
        // A fastcap's third part is not the physics.
        assert_eq!(physics_from_filename("cpm22[fc.cpm.3]12.345(nick).dm_68").as_deref(), Some("cpm"));
        assert_eq!(physics_from_filename("my run.dm_68"), None);
        assert_eq!(physics_from_filename("cpm22[df]12.345(nick).dm_68"), None);
    }

    #[test]
    fn a_finished_round_holds_nothing() {
        let state = CompsState::default();
        state.adopt(payload("Fast-Strafe", &past()));
        assert!(state.guard_match("fast-strafe[df.cpm]01.234(nick).dm_68").is_none());
        assert!(state.open_round_id().is_none());
    }

    #[test]
    fn no_payload_means_no_guard() {
        let state = CompsState::default();
        assert!(state.guard_match("fast-strafe[df.cpm]01.234(nick).dm_68").is_none());
    }
}
