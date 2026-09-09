//! On-disk state: a single JSON file, written atomically (tmp + rename).
//! Volume is tiny (hundreds of problems, ≤8k cached submissions), so a plain
//! document beats an embedded database for footprint and portability.

use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "state.json";

const MAX_SUBMISSIONS: usize = 8_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    pub handle: String,
    /// Refresh only happens on explicit request (TLE-style). Off by default.
    pub refresh_on_open: bool,
    pub start_minimized: bool,
    pub launch_on_login: bool,
    /// User's UTC offset in minutes; resolved from the system on first run and
    /// frozen so streak-day boundaries stay stable. `None` until first launch.
    pub utc_offset_minutes: Option<i32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            handle: String::new(),
            utc_offset_minutes: None,
            refresh_on_open: false,
            start_minimized: false,
            launch_on_login: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Data {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub packs: Vec<Pack>,
    #[serde(default)]
    pub programs: Vec<Program>,
    #[serde(default, with = "solved_map_ser")]
    pub solved: SolvedMap,
    #[serde(default)]
    pub profile: Option<CfProfile>,
    /// Raw submission cache. Serialized ONLY into `submissions.json`
    /// (managed by Store); never embedded in state.json again.
    #[serde(default, skip_serializing)]
    pub submissions: Vec<Submission>,
    #[serde(default)]
    pub rating_history: Vec<RatingPoint>,
    #[serde(default)]
    pub last_sync: Option<i64>,
    /// Handle the solved/submission/rating caches belong to. When it drifts
    /// from `settings.handle`, the caches are wiped before the next sync so a
    /// previous account's progress can never leak into the new one.
    #[serde(default)]
    pub solved_for: Option<String>,
    /// Per-problem editor buffers, keyed by [`editor_key`] (`"contestId/index"`).
    #[serde(default)]
    pub editor_files: BTreeMap<String, EditorFile>,
    /// Sample tests scraped from Codeforces statements, same keying.
    /// Cached so local runs work offline after the first fetch.
    #[serde(default)]
    pub editor_samples: BTreeMap<String, Vec<Sample>>,
}

impl Default for Data {
    fn default() -> Self {
        Self { version: 1, settings: Settings::default(), packs: Vec::new(), programs: Vec::new(), solved: SolvedMap::default(), submissions: Vec::new(), profile: None, rating_history: Vec::new(), last_sync: None, solved_for: None, editor_files: BTreeMap::new(), editor_samples: BTreeMap::new() }
    }
}

pub struct Store {
    pub path: PathBuf,
    /// Submission-cache sidecar (`submissions.json`). Kept out of the startup
    /// path: the UI never reads raw submissions — only the solved map does.
    subs_path: PathBuf,
    subs_loaded: bool,
    pub data: Data,
}

const SUBS_FILE: &str = "submissions.json";

impl Store {
    /// Loads `dir/state.json` (small). The submission cache is NOT read here;
    /// call [`Store::ensure_subs`] before touching `data.submissions`.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(STATE_FILE);
        let mut data = fs::read(&path).ok().and_then(|bytes| {
            match serde_json::from_slice::<Data>(&bytes) {
                Ok(d) => Some(d),
                Err(_) => {
                    let _ = fs::rename(&path, dir.join("state.corrupt.json"));
                    None
                }
            }
        }).unwrap_or_default();

        // One-time migration: legacy state.json embedded the cache inline.
        let legacy = std::mem::take(&mut data.submissions);
        let subs_path = dir.join(SUBS_FILE);
        if !legacy.is_empty() && !subs_path.exists() {
            if let Ok(bytes) = serde_json::to_vec(&legacy) {
                let _ = fs::write(&subs_path, bytes);
            }
        }

        // Pre-provenance states (built before `solved_for` existed) may hold
        // another account's progress — e.g. after QA handle swaps. Unknown
        // origin + non-empty caches: drop them once; the next sync rebuilds.
        if data.solved_for.is_none()
            && (!data.solved.is_empty() || !data.rating_history.is_empty())
        {
            data.solved.clear();
            data.rating_history.clear();
            data.profile = None;
            data.last_sync = None;
        }

        Self { path, subs_path, subs_loaded: false, data }
    }

    /// Materializes the submission cache from its sidecar (once).
    pub fn ensure_subs(&mut self) {
        if self.subs_loaded {
            return;
        }
        self.subs_loaded = true;
        self.data.submissions = fs::read(&self.subs_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    }

    fn write_subs(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.data.submissions).map_err(|e| e.to_string())?;
        let tmp = self.subs_path.with_extension("tmp");
        fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        if self.subs_path.exists() {
            let _ = fs::remove_file(&self.subs_path);
        }
        fs::rename(&tmp, &self.subs_path).map_err(|e| e.to_string())
    }

    /// Persists core state and (when materialized) the submission cache.
    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let bytes = serde_json::to_vec(&self.data).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        if self.path.exists() {
            let _ = fs::remove_file(&self.path);
        }
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        if self.subs_loaded {
            self.write_subs()?;
        }
        Ok(())
    }

    /// Merges one sync's results. Returns how many *new* problems were solved.
    ///
    /// `handle` is the account this sync belongs to. If the cached progress
    /// was built for a different handle (user switched accounts), every
    /// derived cache is wiped first — stale ACs must never survive a switch.
    pub fn apply_sync(
        &mut self,
        handle: &str,
        profile: CfProfile,
        ratings: Vec<RatingPoint>,
        subs: &[Submission],
        now: i64,
        utc_offset_minutes: i32,
    ) -> usize {
        self.ensure_subs();
        if self.data.solved_for.as_deref() != Some(handle) {
            self.data.solved.clear();
            self.data.submissions.clear();
            self.data.rating_history.clear();
            self.data.profile = None;
            self.data.last_sync = None;
            self.data.solved_for = Some(handle.to_string());
        }
        self.data.profile = Some(profile);
        if !ratings.is_empty() {
            self.data.rating_history = ratings;
        }
        let mut new_solved = 0usize;
        let mut seen: HashSet<i64> = self.data.submissions.iter().map(|s| s.id).collect();
        for s in subs {
            if seen.insert(s.id) {
                self.data.submissions.push(s.clone());
            }
            if s.verdict == "OK" {
                let day = crate::engine::day_ord_of_ts(s.ts, utc_offset_minutes);
                match self.data.solved.get_mut(&s.key) {
                    Some(info) => {
                        if s.ts < info.ts {
                            info.ts = s.ts;
                            info.day = day;
                        }
                    }
                    None => {
                        self.data.solved.insert(s.key.clone(), SolvedInfo { ts: s.ts, day });
                        new_solved += 1;
                    }
                }
            }
        }
        self.data.submissions.sort_by_key(|s| std::cmp::Reverse(s.ts));
        self.data.submissions.truncate(MAX_SUBMISSIONS);
        self.data.last_sync = Some(now);
        new_solved
    }

    pub fn pack_by_id(&self, id: &str) -> Option<&Pack> {
        self.data.packs.iter().find(|p| p.id == id)
    }
}

/// Solved map uses a struct key, which JSON can't represent as an object key;
/// serialize it as a list of `(key, info)` pairs instead.
mod solved_map_ser {
    use crate::model::{ProblemKey, SolvedInfo, SolvedMap};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(m: &SolvedMap, s: S) -> Result<S::Ok, S::Error> {
        let pairs: Vec<(ProblemKey, SolvedInfo)> =
            m.iter().map(|(k, v)| (k.clone(), *v)).collect();
        pairs.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SolvedMap, D::Error> {
        let pairs = Vec::<(ProblemKey, SolvedInfo)>::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cf::{fixtures, parse_profile, parse_submissions};
    use crate::model::ProblemKey;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rtom-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sync_for(st: &mut Store, handle: &str, keys: &[(ProblemKey, i64, i64)]) -> usize {
        let profile = parse_profile(&fixtures::profile_json(handle, 1500)).unwrap();
        let subs = parse_submissions(&fixtures::status_json(keys)).unwrap();
        st.apply_sync(handle, profile, Vec::new(), &subs, 1_700_000_000, 0)
    }

    #[test]
    fn switching_handles_wipes_previous_progress() {
        let dir = temp_dir("switch");
        let mut st = Store::load(&dir);
        st.data.settings.handle = "alice".into();

        let k1 = ProblemKey::new(1_i64, "A");
        assert_eq!(
            sync_for(&mut st, "alice", &[(k1.clone(), 1, 4), (ProblemKey::new(2_i64, "B"), 2, 5)]),
            2
        );
        assert_eq!(st.data.solved.len(), 2);

        // Switch to bob: alice's ACs must not leak into the new account view.
        st.data.settings.handle = "bob".into();
        let k3 = ProblemKey::new(3_i64, "C");
        assert_eq!(sync_for(&mut st, "bob", &[(k3.clone(), 5, 8)]), 1);
        assert_eq!(st.data.solved.len(), 1);
        assert!(st.data.solved.contains_key(&k3));
        assert_eq!(st.data.profile.as_ref().unwrap().handle, "bob");
    }

    #[test]
    fn resync_same_handle_keeps_accumulating() {
        let dir = temp_dir("same");
        let mut st = Store::load(&dir);
        let ka = ProblemKey::new(10_i64, "A");
        let kb = ProblemKey::new(11_i64, "B");
        assert_eq!(sync_for(&mut st, "alice", &[(ka.clone(), 3, 6)]), 1);
        assert_eq!(sync_for(&mut st, "alice", &[(ka.clone(), 3, 6), (kb, 4, 7)]), 1); // only kb new
        assert_eq!(st.data.solved.len(), 2);
    }

    #[test]
    fn load_drops_unprovenanced_caches_once() {
        let dir = temp_dir("legacy");
        let mut st = Store::load(&dir);
        let k = ProblemKey::new(7_i64, "A");
        sync_for(&mut st, "alice", &[(k.clone(), 9, 9)]);
        let _ = st.save();

        // Simulate a pre-provenance state file (no solved_for field).
        let mut raw = serde_json::to_value(&st.data).unwrap();
        raw.as_object_mut().unwrap().remove("solved_for");
        std::fs::write(
            dir.join("state.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();

        let reloaded = Store::load(&dir);
        assert!(reloaded.data.solved.is_empty());
        assert!(reloaded.data.profile.is_none());
        assert!(reloaded.data.last_sync.is_none());
    }
}
