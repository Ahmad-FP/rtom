//! Domain models. UI-free and IO-free so they are portable (future mobile)
//! and trivially deterministic under test/bench.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Codeforces problem identity.
#[derive(
    Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug,
)]
pub struct ProblemKey {
    pub contest_id: i64,
    pub index: String,
}

impl ProblemKey {
    pub fn new(contest_id: i64, index: impl Into<String>) -> Self {
        Self { contest_id, index: index.into() }
    }
    /// URL for gym-style ids would differ, but CP-31 sheets only use regular contests.
    pub fn url(&self) -> String {
        format!(
            "https://codeforces.com/contest/{}/problem/{}",
            self.contest_id, self.index
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Problem {
    pub key: ProblemKey,
    pub name: String,
    pub rating: i64,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Sheet {
    pub rating: i64,
    #[serde(default)]
    pub problems: Vec<Problem>,
}

/// A curated problem collection (e.g. CP-31).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub sheets: Vec<Sheet>,
}

impl Pack {
    pub fn total_problems(&self) -> usize {
        self.sheets.iter().map(|s| s.problems.len()).sum()
    }

    /// Problems restricted to `order` (sheet ratings, ascending order preserved),
    /// ordered easiest-sheet first, then by problem rating, then key.
    pub fn scope_problems(&self, order: &[i64]) -> Vec<&Problem> {
        let mut out: Vec<&Problem> = self
            .sheets
            .iter()
            .filter(|s| order.contains(&s.rating))
            .flat_map(|s| s.problems.iter())
            .collect();
        out.sort_by_key(|p| (p.rating, p.key.clone()));
        out
    }

    pub fn find(&self, key: &ProblemKey) -> Option<&Problem> {
        self.sheets
            .iter()
            .find_map(|s| s.problems.iter().find(|p| &p.key == key))
    }
}

/// A deadline tied to a cumulative completion percentage ("25% by day 14").
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Milestone {
    pub label: String,
    /// Days after program start.
    pub due_day: i32,
    /// Cumulative percent of program scope that should be solved by the deadline (0..=100).
    pub cum_pct: f32,
}

/// A training program: a pack scope + a schedule of percentage milestones.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Program {
    pub id: String,
    pub name: String,
    pub pack_id: String,
    /// Which sheet ratings belong to this program (e.g. 800..=1900 step 100).
    pub sheet_order: Vec<i64>,
    /// Program start as days-since-CE (chrono NaiveDate::num_days_from_ce).
    pub start_day: i32,
    pub duration_days: i32,
    pub milestones: Vec<Milestone>,
    pub created_at: i64,
}

impl Program {
    pub fn date_of(&self, due_day: i32) -> NaiveDate {
        date_from_ce(self.start_day + due_day)
    }
    pub fn end_date(&self) -> NaiveDate {
        self.date_of(self.duration_days)
    }
    pub fn active_or_upcoming(&self, today: NaiveDate) -> bool {
        today <= self.end_date()
    }
}

pub fn date_from_ce(days: i32) -> NaiveDate {
    NaiveDate::from_num_days_from_ce_opt(days)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
}

pub fn today_ord(date: NaiveDate) -> i32 {
    date.num_days_from_ce()
}

/// First-solve record for a problem.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct SolvedInfo {
    pub ts: i64,
    pub day: i32,
}

pub type SolvedMap = BTreeMap<ProblemKey, SolvedInfo>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Submission {
    pub id: i64,
    pub ts: i64,
    pub key: ProblemKey,
    pub verdict: String,
    pub rating: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CfProfile {
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub rating: Option<i64>,
    #[serde(default)]
    pub max_rating: Option<i64>,
    #[serde(default)]
    pub rank: String,
    #[serde(default)]
    pub max_rank: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RatingPoint {
    pub ts: i64,
    pub rating: i64,
    pub delta: i64,
    pub contest: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Streak {
    pub current: i32,
    pub longest: i32,
    pub total_days: i32,
}

/// Set of ordinals for days with at least one AC.
pub type ActiveDays = BTreeSet<i32>;

pub struct RankTier {
    pub name: &'static str,
    pub color: [u8; 3],
}
