//! Pure logic: streaks, ranks, XP, the standard CP-31 program and milestone evaluation.
//! Every function takes explicit inputs (no clock, no IO) so the benchmark stays deterministic.

use crate::model::*;
use chrono::{NaiveDate, TimeZone};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Dates / formatting
// ---------------------------------------------------------------------------

/// 1970-01-01 expressed as a CE ordinal (chrono anchor).
pub const EPOCH_DAY_CE: i32 = 719_163;

/// User-local calendar day of a timestamp, given the user's UTC offset in minutes.
/// Pure and deterministic — no chrono-Local anywhere in engine/store.
pub fn day_ord_of_ts(ts: i64, utc_offset_minutes: i32) -> i32 {
    EPOCH_DAY_CE + (ts + utc_offset_minutes as i64 * 60).div_euclid(86_400) as i32
}

/// Start-of-day unix seconds for a CE ordinal under the same offset.
pub fn day_start_ts(ord: i32, utc_offset_minutes: i32) -> i64 {
    (ord - EPOCH_DAY_CE) as i64 * 86_400 - utc_offset_minutes as i64 * 60
}

/// "Jul 12" for a unix timestamp (UTC — display only).
pub fn fmt_ts_date(ts: i64) -> String {
    chrono::Utc
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.date_naive().format("%b %d").to_string())
        .unwrap_or_default()
}

pub fn fmt_duration(secs: i64) -> String {
    if secs <= 0 {
        return "overdue".into();
    }
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        let m = (secs % 3_600) / 60;
        format!("{h}h {m}m")
    }
}

pub fn fmt_date(d: NaiveDate) -> String {
    d.format("%b %d").to_string()
}

// ---------------------------------------------------------------------------
// Ranks & XP
// ---------------------------------------------------------------------------

/// Codeforces rating → tier (colors brightened for dark UI).
pub fn cf_rank(rating: Option<i64>) -> RankTier {
    let Some(r) = rating else {
        return RankTier { name: "unrated", color: [138, 151, 171] };
    };
    let (name, color) = match r {
        0..=1199 => ("newbie", [160, 166, 178]),
        1200..=1399 => ("pupil", [80, 214, 144]),
        1400..=1599 => ("specialist", [62, 216, 200]),
        1600..=1899 => ("expert", [108, 168, 255]),
        1900..=2099 => ("candidate master", [196, 130, 252]),
        2100..=2299 => ("master", [244, 114, 182]),
        2300..=2399 => ("international master", [244, 114, 182]),
        2400..=2499 => ("grandmaster", [255, 106, 106]),
        _ => ("legendary grandmaster", [255, 138, 138]),
    };
    RankTier { name, color }
}

/// Local ladder: CF-flavored names driven by effort, not live rating.
const LOCAL_TIERS: &[(u64, &str)] = &[
    (0, "newbie"),
    (400, "pupil"),
    (900, "specialist"),
    (1_600, "expert"),
    (2_600, "candidate master"),
    (4_000, "master"),
    (6_000, "international master"),
    (9_000, "grandmaster"),
    (13_500, "international grandmaster"),
];

pub fn xp_of(solved: usize, longest_streak: i32, milestones_done: i32, current_streak: i32) -> u64 {
    solved as u64 * 10
        + longest_streak as u64 * 25
        + milestones_done as u64 * 150
        + current_streak.min(60) as u64 * 5
}

/// (tier name, xp into tier, tier width). Last tier reports width 5_000 capped for display.
pub fn local_rank(xp: u64) -> (&'static str, u64, u64) {
    for w in LOCAL_TIERS.windows(2) {
        let (lo, name) = w[0];
        let (hi, _) = w[1];
        if xp < hi {
            return (name, xp - lo, hi - lo);
        }
    }
    let (last, name) = LOCAL_TIERS[LOCAL_TIERS.len() - 1];
    (name, (xp - last).min(5_000), 5_000)
}

// ---------------------------------------------------------------------------
// Streaks
// ---------------------------------------------------------------------------

/// Streaks from a set of AC-day ordinals, evaluated against `today`.
pub fn compute_streak(days: &BTreeSet<i32>, today: i32) -> Streak {
    let total_days = days.len() as i32;
    if days.is_empty() {
        return Streak::default();
    }
    let sorted: Vec<i32> = days.iter().copied().collect();
    let mut longest = 1;
    let mut run = 1;
    for w in sorted.windows(2) {
        if w[1] == w[0] + 1 {
            run += 1;
        } else {
            run = 1;
        }
        longest = longest.max(run);
    }
    // Current streak ends today or yesterday (grace for "haven't solved yet today").
    let mut cur = 0i32;
    let anchor = if days.contains(&today) {
        today
    } else if days.contains(&(today - 1)) {
        today - 1
    } else {
        return Streak { current: 0, longest, total_days };
    };
    let mut d = anchor;
    while days.contains(&d) {
        cur += 1;
        d -= 1;
    }
    Streak { current: cur, longest, total_days }
}

// ---------------------------------------------------------------------------
// Standard CP-31 program (the default training pack goal)
// ---------------------------------------------------------------------------

/// Sheets covered by the standard goal: everything up to 1900.
pub const CP31_STANDARD_MAX_RATING: i64 = 1900;

/// Builds "clear CP-31 up to 1900 in 8 weekly milestones over 56 days".
/// Problems are ordered easy-sheet-first, so equal cumulative-percentage steps
/// front-load easier sheets and naturally ramp difficulty.
pub fn standard_cp31_program(pack: &Pack, start: NaiveDate, now_ts: i64) -> Program {
    let mut order: Vec<i64> = pack
        .sheets
        .iter()
        .map(|s| s.rating)
        .filter(|r| *r <= CP31_STANDARD_MAX_RATING)
        .collect();
    order.sort_unstable();

    let weeks: i32 = 8;
    let duration = weeks * 7;
    let scope_len = pack.scope_problems(&order).len() as f32;

    let mut milestones = Vec::with_capacity(weeks as usize);
    for k in 1..=weeks {
        let pct = 100.0 * k as f32 / weeks as f32;
        let label = week_span_label(pack, &order, scope_len, k, weeks);
        milestones.push(Milestone {
            label,
            due_day: duration / weeks * k,
            cum_pct: pct,
        });
    }

    Program {
        id: "cp31-standard".into(),
        name: "CP-31 · clear up to 1900".into(),
        pack_id: pack.id.clone(),
        sheet_order: order,
        start_day: today_ord(start),
        duration_days: duration,
        milestones,
        created_at: now_ts,
    }
}
#[derive(Clone, Debug)]
pub struct MilestoneView {
    pub idx: usize,
    pub label: String,
    pub due: NaiveDate,
    pub cum_pct: f32,
    pub target: usize,
    /// Problems solved in scope as of the evaluation day.
    pub now: usize,
    pub state: MilestoneState,
}

fn week_span_label(
    pack: &Pack,
    order: &[i64],
    total: f32,
    week: i32,
    weeks: i32,
) -> String {
    if total < 1.0 || order.is_empty() {
        return format!("Week {week}");
    }
    let n = total as usize;
    let lo = n * (week as usize - 1) / weeks as usize;
    let hi = (n * week as usize / weeks as usize).saturating_sub(1).max(lo);
    let scope = pack.scope_problems(order);
    let r_lo = scope[lo.min(n - 1)].rating;
    let r_hi = scope[hi.min(n - 1)].rating;
    if r_lo == r_hi {
        format!("Week {week} · sheet {r_lo}")
    } else {
        format!("Week {week} · sheets {r_lo}–{r_hi}")
    }
}

// ---------------------------------------------------------------------------
// Program evaluation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum MilestoneState {
    /// Program hasn't started yet.
    Future,
    /// In progress; `deficit` = problems behind interpolated schedule (0 when on/ahead).
    Active { on_track: bool, deficit: i64, days_left: i32 },
    Done { late: bool },
    /// Deadline passed without reaching target.
    Missed { deficit: i64 },
}


#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PrgStatus {
    OnTrack,
    Ahead,
    Behind,
    Complete,
}

#[derive(Clone, Debug)]
pub struct ProgramProgress {
    pub solved_count: usize,
    pub total: usize,
    pub pct: f32,
    pub views: Vec<MilestoneView>,
    pub current_idx: Option<usize>,
    pub done_milestones: usize,
    pub status: PrgStatus,
}

impl Default for ProgramProgress {
    fn default() -> Self {
        Self {
            solved_count: 0,
            total: 0,
            pct: 0.0,
            views: Vec::new(),
            current_idx: None,
            done_milestones: 0,
            status: PrgStatus::OnTrack,
        }
    }
}

/// Evaluates a program against the solved map. `None` when the scope is empty.
pub fn evaluate(program: &Program, pack: &Pack, solved: &SolvedMap, today: NaiveDate) -> Option<ProgramProgress> {
    let scope = pack.scope_problems(&program.sheet_order);
    let total = scope.len();
    if total == 0 {
        return None;
    }
    let today_o = today_ord(today);
    let start_o = program.start_day;

    let mut count = 0usize;
    let mut solved_days: Vec<i32> = Vec::new();
    for p in &scope {
        if let Some(s) = solved.get(&p.key) {
            count += 1;
            solved_days.push(s.day);
        }
    }
    solved_days.sort_unstable();
    let count_at = |day: i32| solved_days.partition_point(|&d| d <= day);

    let pct = count as f32 / total as f32 * 100.0;

    let mut views: Vec<MilestoneView> = Vec::with_capacity(program.milestones.len());
    let mut done_milestones = 0usize;
    let mut current_idx: Option<usize> = None;
    let mut behind_now = false;
    let mut ahead_now = false;

    for (idx, m) in program.milestones.iter().enumerate() {
        let due_o = start_o + m.due_day.max(0);
        let target = ((m.cum_pct / 100.0) * total as f32).ceil() as usize;
        let due = program.date_of(m.due_day.max(0));
        let mut behind_this = false;
        let mut ahead_this = false;
        let mut state = if today_o < start_o {
            MilestoneState::Future
        } else if count_at(due_o) >= target {
            done_milestones += 1;
            MilestoneState::Done { late: false }
        } else if today_o > due_o && count_at(today_o) >= target {
            done_milestones += 1;
            MilestoneState::Done { late: true }
        } else if today_o > due_o {
            let deficit = (target.saturating_sub(count_at(due_o))) as i64;
            if deficit > 0 {
                behind_now = true;
            }
            MilestoneState::Missed { deficit }
        } else {
            // Active: interpolate an expected count between the previous
            // milestone and this one. Floor (never ceil) so fractional days
            // don't manufacture debt, and grant a grace window at the start of
            // each milestone: being a day or two behind pace early on is not
            // "behind schedule" — missing whole days past grace is.
            let (prev_due_o, prev_target): (i32, usize) = match views.last() {
                Some(prev) => (
                    start_o + program.milestones[prev.idx].due_day.max(0),
                    ((prev.cum_pct / 100.0) * total as f32).ceil() as usize,
                ),
                None => (start_o, 0),
            };
            let span = (due_o - prev_due_o).max(1);
            let days_in = today_o - prev_due_o;
            let frac = ((today_o - prev_due_o) as f32 / span as f32).clamp(0.0, 1.0);
            let expected =
                (prev_target as f32 + frac * (target - prev_target) as f32).floor();
            let raw_deficit = (expected as i64 - count as i64).max(0);
            // Grace covers ~15% of the window and forgives up to that many
            // days of pace, so weekly goals tolerate a slow start but still
            // bite well before the deadline.
            let grace_days = (((span as f32) * 0.15).ceil() as i64).max(1);
            let daily_rate = (target - prev_target) as f32 / span as f32;
            let within_grace = days_in <= grace_days as i32
                && raw_deficit as f32 <= daily_rate * grace_days as f32;
            let deficit = if within_grace { 0 } else { raw_deficit };
            behind_this = deficit > 0;
            ahead_this = !behind_this && count as f32 > expected + 1.0;
            MilestoneState::Active { on_track: deficit == 0, deficit, days_left: due_o - today_o }
         };
 
         // Only ONE milestone can be Active: the first incomplete one whose
         // window contains today. Everything later is Future.

         if matches!(state, MilestoneState::Active { .. }) {
             if current_idx.is_none() {
                 current_idx = Some(idx);

             } else {
                 state = MilestoneState::Future;

             }
        } else {

        }

        // Pace flags only count for the milestone that stayed Active.
        if let MilestoneState::Active { .. } = state {
            if behind_this {
                behind_now = true;
            } else if ahead_this {
                ahead_now = true;
            }
         }


        views.push(MilestoneView {
            idx,
            label: m.label.clone(),
            due,
            cum_pct: m.cum_pct,
            target,
            now: count_at(today_o),
            state,
        });
    }
    // Fallbacks: before start → first milestone; everything past → last.
    if current_idx.is_none() {
        current_idx = if today_o < start_o {
            views.first().map(|v| v.idx)
        } else {
            views.last().map(|v| v.idx)
        };
    }

    let status = if pct >= 99.9 {
        PrgStatus::Complete
    } else if behind_now {
        PrgStatus::Behind
    } else if ahead_now {
        PrgStatus::Ahead
    } else {
        PrgStatus::OnTrack
    };

    Some(ProgramProgress { solved_count: count, total, pct, views, current_idx, done_milestones, status })
}
