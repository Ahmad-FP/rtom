//! App shell: theme, shared widgets, navigation, sync orchestration.
//! The whole UI lives in `peng_lib` so the benchmark can drive the exact
//! same code paths headlessly (no window needed).

pub mod dashboard;
pub mod editor;
pub mod program_detail;
pub mod problems;
pub mod programs;
pub mod settings;

use crate::cf::{AuthOutcome, CfClient, SubmitOutcome, SyncOutcome};
use crate::engine::{self, ProgramProgress};
use crate::model::{date_from_ce, ActiveDays, Streak};
use crate::platform;
use crate::store::Store;
use chrono::Utc;
use egui::{
    Align2, Color32, Context, FontId, Margin, Pos2, RichText, Sense, Shape, Stroke, TextStyle, Vec2, CornerRadius, ThemePreference,
};
use std::collections::BTreeSet;
use std::sync::mpsc::{channel, Receiver, Sender};

// ---------------------------------------------------------------------------
// Theme — soft cool dark, personal rather than SaaS
// ---------------------------------------------------------------------------

pub mod theme {
    use super::*;
    pub const BG: Color32 = Color32::from_rgb(15, 19, 26);
    pub const PANEL: Color32 = Color32::from_rgb(21, 27, 37);
    pub const CARD: Color32 = Color32::from_rgb(27, 35, 48);
    pub const CARD_HI: Color32 = Color32::from_rgb(34, 43, 59);
    pub const STROKE: Color32 = Color32::from_rgb(44, 55, 73);
    pub const TEXT: Color32 = Color32::from_rgb(216, 226, 240);
    pub const DIM: Color32 = Color32::from_rgb(139, 151, 171);
    pub const FAINT: Color32 = Color32::from_rgb(97, 108, 128);
    pub const ACCENT: Color32 = Color32::from_rgb(94, 234, 212); // teal
    pub const ACCENT2: Color32 = Color32::from_rgb(96, 165, 250); // soft blue
    pub const GOOD: Color32 = Color32::from_rgb(80, 214, 144);
    pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
    pub const BAD: Color32 = Color32::from_rgb(248, 113, 113);
    pub const FLAME: Color32 = Color32::from_rgb(251, 146, 60);

    pub fn install(ctx: &Context) {
        ctx.set_theme(ThemePreference::Dark);
        ctx.all_styles_mut(|st| {
            let v = &mut st.visuals;
            v.panel_fill = PANEL;
            v.window_fill = CARD;
            v.extreme_bg_color = Color32::from_rgb(11, 14, 20);
            v.window_corner_radius = CornerRadius::same(14);
            v.hyperlink_color = ACCENT2;
            v.selection.bg_fill =
                Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 46);
            v.selection.stroke = Stroke::new(1.0, ACCENT);
            let w = &mut v.widgets;
            w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
            w.inactive.corner_radius = CornerRadius::same(9);
            w.inactive.weak_bg_fill = CARD;
            w.inactive.bg_fill = CARD;
            w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
            w.hovered.corner_radius = CornerRadius::same(9);
            w.hovered.weak_bg_fill = CARD_HI;
            w.hovered.bg_fill = CARD_HI;
            w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
            w.active.corner_radius = CornerRadius::same(9);
            w.active.weak_bg_fill = Color32::from_rgb(41, 52, 70);
            w.active.bg_fill = Color32::from_rgb(41, 52, 70);
            st.text_styles = [
                (TextStyle::Body, FontId::proportional(15.0)),
                (TextStyle::Button, FontId::proportional(15.0)),
                (TextStyle::Heading, FontId::proportional(26.0)),
                (TextStyle::Small, FontId::proportional(12.0)),
                (TextStyle::Monospace, FontId::monospace(13.5)),
            ]
            .into();
            st.spacing.item_spacing = Vec2::new(10.0, 10.0);
            st.spacing.button_padding = Vec2::new(12.0, 6.0);
        });
    }
}

// ---------------------------------------------------------------------------
// Shared widgets
// ---------------------------------------------------------------------------

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(13))
        .stroke(Stroke::new(1.0_f32, theme::STROKE))
        .inner_margin(Margin::symmetric(15, 13))
        .show(ui, |ui| add(ui))
        .inner
}

/// Horizontal progress bar painted directly (consistent look everywhere).
pub fn bar(ui: &mut egui::Ui, frac: f32, width: f32, height: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let rounding = CornerRadius::same((height / 2.0) as u8);
    p.rect_filled(rect, rounding, theme::STROKE);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.003 {
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height())),
            rounding,
            color,
        );
    }
}

/// Progress ring; `frac` 0..1.
pub fn ring(ui: &mut egui::Ui, size: f32, frac: f32, color: Color32, label: String, sub: String) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let center = rect.center();
    let radius = size / 2.0 - size * 0.07;
    let width = size * 0.085;
    let p = ui.painter();
    p.circle_stroke(center, radius, Stroke::new(width, theme::STROKE));
    let f = frac.clamp(0.0, 1.0);
    if f >= 0.999 {
        p.circle_stroke(center, radius, Stroke::new(width, color));
    } else if f > 0.001 {
        let sweep = std::f32::consts::TAU * f;
        let steps = ((sweep / (std::f32::consts::TAU / 64.0)).ceil() as usize).max(2);
        let pts: Vec<egui::Pos2> = (0..=steps)
            .map(|k| {
                let a = -std::f32::consts::FRAC_PI_2 + sweep * k as f32 / steps as f32;
                center + radius * egui::vec2(a.cos(), a.sin())
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(width, color)));
    }
    p.text(
        center + Vec2::new(0.0, -size * 0.06),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(size * 0.24),
        theme::TEXT,
    );
    p.text(
        center + Vec2::new(0.0, size * 0.13),
        Align2::CENTER_CENTER,
        sub,
        FontId::proportional(size * 0.115),
        theme::DIM,
    );
}

pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    let pad = Vec2::new(4.0, 2.0);
    let galley =
        ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.5), color);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(
        rect,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 30),
    );
    p.galley(rect.min + pad * 0.5, galley, color);
}

pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    chip(ui, text, color);
}

/// Streak chip with a *painted* flame (default fonts have no emoji coverage).
pub fn flame_chip(ui: &mut egui::Ui, days: i32) {
    let text = format!("{days}-day streak");
    let galley =
        ui.painter().layout_no_wrap(text, FontId::proportional(12.5), theme::FLAME);
    let icon = 15.0;
    let size = egui::vec2(galley.size().x + icon + 12.0, galley.size().y.max(icon) + 5.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(
        rect,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(theme::FLAME.r(), theme::FLAME.g(), theme::FLAME.b(), 30),
    );
    let c = rect.min + egui::vec2(icon * 0.5 + 3.0, rect.height() * 0.52);
    let s = icon * 0.40;
    let outer = [
        c + s * egui::vec2(-0.55, 0.90),
        c + s * egui::vec2(0.55, 0.90),
        c + s * egui::vec2(0.75, -0.10),
        c + s * egui::vec2(0.00, -1.10),
    ];
    p.add(Shape::convex_polygon(
        outer.to_vec(),
        theme::FLAME,
        Stroke::NONE,
    ));
    let inner = [
        c + s * egui::vec2(-0.22, 0.85),
        c + s * egui::vec2(0.22, 0.85),
        c + s * egui::vec2(0.30, 0.18),
        c + s * egui::vec2(0.00, -0.30),
    ];
    p.add(Shape::convex_polygon(
        inner.to_vec(),
        theme::WARN,
        Stroke::NONE,
    ));
    p.galley(
        rect.min + egui::vec2(icon + 7.0, (rect.height() - galley.size().y) * 0.5),
        galley,
        theme::FLAME,
    );
}

pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(38.0, 21.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = ui.painter_at(rect.expand(2.0));
    let track = if *on { theme::ACCENT } else { theme::STROKE };
    p.rect_filled(rect, CornerRadius::same(11), track);
    let knob_x = if *on { rect.right() - 11.0 } else { rect.left() + 11.0 };
    p.circle_filled(Pos2::new(knob_x, rect.center().y), 7.5, Color32::WHITE);
    resp
}

pub fn stat_card(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
    card(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(value).size(23.0).strong().color(color));
            ui.label(RichText::new(label.to_uppercase()).size(11.0).color(theme::DIM));
        });
    });
}

pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).heading());
    ui.add_space(2.0);
}

pub fn rel_time(ts: i64, now: i64) -> String {
    let d = now - ts;
    if d < 45 {
        "just now".into()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86_400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86_400)
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Programs,
    Problems,
    Settings,
    /// Full page for one training program (payload: program id).
    ProgramDetail(String),
    /// Code editor for one problem (payload: `"contestId/index"`).
    Editor(String),
}


#[derive(Clone, Default)]
pub struct DashCache {
    pub progress: Option<ProgramProgress>,
    pub program_id: String,
    pub program_name: String,
    pub streak: Streak,
    /// Start date of the primary program ("the day you began").
    pub program_start: Option<chrono::NaiveDate>,
    pub xp: u64,
    pub rank_name: &'static str,
    pub rank_into: u64,
    pub rank_width: u64,
    pub solved_total: usize,
    pub solved_today: usize,
    pub week_solves: usize,
    /// Streak >= 2 and nothing solved today: today's streak will die at midnight.
    pub streak_at_risk: bool,
    /// Seconds until the local-day boundary.
    pub secs_to_midnight: i64,
    pub next_up: Option<String>,
    pub next_up_url: Option<String>,
    pub next_up_key: Option<crate::model::ProblemKey>,
}

pub struct PengApp {
    pub store: Store,
    pub screen: Screen,
    /// Injected clock (real app: system time; bench: deterministic).
    pub now_ts: i64,
    pub today_ord: i32,
    pub sync_tx: Sender<SyncOutcome>,
    sync_rx: Receiver<SyncOutcome>,
    pub syncing: bool,
    pub sync_started_at: i64,
    pub toast: Option<(String, f64, Color32)>,
    dirty: bool,
    pub dash: DashCache,
    pub prob_sheet: usize,
    pub prob_query: String,
    pub prob_only_unsolved: bool,
    pub wizard_open: bool,
    pub draft: programs::Draft,
    pub prog_confirm_del: Option<String>,
    /// Program currently being renamed: (program id, name buffer).
    pub prog_rename: Option<(String, String)>,
    pub handle_buf: String,
    pub import_buf: String,
    /// Week-goal detail popup visibility (Home hero card).
    pub goal_detail_open: bool,
    /// Streak-at-risk toast already shown this session.
    pub at_risk_toast_done: bool,
    /// Frozen user UTC offset (minutes). Resolved on first construction.
    pub utc_off: i32,
    /// Screen to return to from the editor (set when the editor is opened).
    pub editor_return: Screen,
    /// Detected local toolchains, resolved once per session.
    pub toolchains: Option<Vec<crate::runner::Toolchain>>,
    /// Session-only Codeforces login buffers. The password lives here while
    /// typed and is taken (dropped after one use) on login — never persisted.
    pub login_handle_buf: String,
    pub login_pass_buf: String,
    /// Session-only authenticated web session. Memory-only; logout/app close
    /// drops it. Never serialized.
    pub cf_session: Option<crate::cf::WebSession>,
    pub authing: bool,
    auth_tx: Sender<AuthOutcome>,
    auth_rx: Receiver<AuthOutcome>,
    pub submit_state: editor::SubmitState,
    submit_tx: Sender<SubmitOutcome>,
    submit_rx: Receiver<SubmitOutcome>,
    /// Last sample-run report for the open editor (`(editor_key, report)`).
    pub ed_report: Option<(String, crate::runner::RunReport)>,
    /// Last custom-input run for the open editor.
    pub ed_custom: Option<crate::runner::RunReport>,
    pub ed_custom_input: String,
    themed: bool,
}

impl PengApp {
    pub fn new(store: Store) -> Self {
        let (tx, rx) = channel();
        let (auth_tx, auth_rx) = channel();
        let (submit_tx, submit_rx) = channel();
        let now = Utc::now().timestamp();
        let utc_off = match store.data.settings.utc_offset_minutes {
            Some(o) => o,
            None => {
                chrono::Local::now().offset().local_minus_utc() / 60
            }
        };
        let handle_buf = store.data.settings.handle.clone();
        Self {
            store,
            screen: Screen::Dashboard,
            now_ts: now,
            today_ord: engine::day_ord_of_ts(now, utc_off),
            sync_tx: tx,
            sync_rx: rx,
            syncing: false,
            sync_started_at: 0,
            toast: None,
            dirty: true,
            dash: DashCache::default(),
            prob_sheet: 0,
            prob_query: String::new(),
            prob_only_unsolved: true,
            wizard_open: false,
            draft: programs::Draft::default(),
            prog_confirm_del: None,
            prog_rename: None,
            handle_buf,
            import_buf: String::new(),
            goal_detail_open: false,
            at_risk_toast_done: false,
            utc_off,
            editor_return: Screen::Problems,
            toolchains: None,
            login_handle_buf: String::new(),
            login_pass_buf: String::new(),
            cf_session: None,
            authing: false,
            auth_tx,
            auth_rx,
            submit_state: editor::SubmitState::default(),
            submit_tx,
            submit_rx,
            ed_report: None,
            ed_custom: None,
            ed_custom_input: String::new(),
            themed: false,
        }
    }

    /// Open the editor for `key`, remembering where to go back to.
    /// Ensures a persisted buffer exists (prefilled with a template on first open).
    pub fn open_editor(&mut self, key: &crate::model::ProblemKey) {
        if !matches!(self.screen, Screen::Editor(_)) {
            self.editor_return = self.screen.clone();
        }
        let k = crate::model::editor_key(key);
        self.store.data.editor_files.entry(k).or_insert_with(|| {
            crate::model::EditorFile { lang: crate::runner::LANG_CPP.into(), source: crate::runner::template_for(crate::runner::LANG_CPP).into() }
        });
        // A fresh login-password buffer every time the editor opens.
        self.login_pass_buf.clear();
        self.submit_state = editor::SubmitState::default();
        self.screen = Screen::Editor(crate::model::editor_key(key));
        self.mark_dirty();
    }

    /// Bench/test hook to pin time deterministically.
    pub fn set_clock(&mut self, now_ts: i64, today_ord: i32) {
        self.now_ts = now_ts;
        self.today_ord = today_ord;
    }

    fn rebuild_dash(&mut self) {
        let mut d = DashCache::default();
        d.solved_total = self.store.data.solved.len();
        let mut days: ActiveDays = BTreeSet::new();
        let _ = &mut days;
        for s in self.store.data.solved.values() {
            days.insert(s.day);
            if s.day == self.today_ord {
                d.solved_today += 1;
            }
            if s.day > self.today_ord - 7 {
                d.week_solves += 1;
            }
        }

        // Primary program = most recently created.
        let prog = self
            .store
            .data
            .programs
            .iter()
            .max_by_key(|p| (p.created_at, p.id.clone()))
            .cloned();

        if let Some(prog) = prog {
            if let Some(pack) = self.store.pack_by_id(&prog.pack_id) {
                let today = date_from_ce(self.today_ord);
                if let Some(pr) =
                    engine::evaluate(&prog, pack, &self.store.data.solved, today)
                {
                    d.streak = engine::compute_streak(&days, self.today_ord);
                    if d.streak.current >= 2 {
                        if let Some(last_day) = days.iter().next_back().copied() {
                            if last_day < self.today_ord {
                                d.streak_at_risk = true;
                                d.secs_to_midnight =
                                    engine::day_start_ts(self.today_ord + 1, self.utc_off)
                                        - self.now_ts;
                            }
                        }
                    }
                    d.program_id = prog.id.clone();
                    d.program_name = prog.name.clone();
                    d.program_start = Some(date_from_ce(prog.start_day));
                    d.xp = engine::xp_of(
                        d.solved_total,
                        d.streak.longest,
                        pr.done_milestones as i32,
                        d.streak.current,
                    );
                    let (name, into, width) = engine::local_rank(d.xp);
                    d.rank_name = name;
                    d.rank_into = into;
                    d.rank_width = width.max(1);
                    // Next unsolved problem in scope.
                    if let Some(p) = pack.scope_problems(&prog.sheet_order).iter().find(|p| {
                        !self.store.data.solved.contains_key(&p.key)
                    }) {
                        d.next_up = Some(format!("{} · {}", p.name, p.rating));
                        d.next_up_url = Some(p.key.url());
                        d.next_up_key = Some(p.key.clone());
                    }
                    d.progress = Some(pr);
                }
            }
        }
        #[cfg(feature = "tray")]
        crate::platform::tray::set_tooltip(&format!(
            "RTOM · {}d streak · {}/{} CP-31",
            d.streak.current,
            d.progress.as_ref().map(|p| p.solved_count).unwrap_or(0),
            d.progress.as_ref().map(|p| p.total).unwrap_or(0)
        ));

        // Debug-only: force the at-risk state for visual QA.
        #[cfg(debug_assertions)]
        if std::env::var("PENG_RISK").is_ok() {
            d.streak_at_risk = true;
            d.streak.current = d.streak.current.max(3);
            d.secs_to_midnight = 7 * 3600 + 1200;
        }
        self.dash = d;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn toast(&mut self, msg: impl Into<String>, color: Color32) {
        self.toast = Some((msg.into(), self.now_ts as f64 + 4.0, color));
    }

    /// Spawns a one-shot blocking fetch on a worker thread (explicit request only).
    pub fn trigger_refresh(&mut self) {
        if self.syncing {
            return;
        }
        let handle = self.store.data.settings.handle.trim().to_string();
        if handle.is_empty() {
            self.toast("Set your Codeforces handle first (Settings)", theme::WARN);
            return;
        }
        self.syncing = true;
        self.sync_started_at = self.now_ts;
        let tx = self.sync_tx.clone();
        std::thread::spawn(move || {
            let client = CfClient::new();
            let outcome = (|| -> Result<SyncOutcome, String> {
                let profile = client.fetch_profile(&handle)?;
                let ratings = client.fetch_ratings(&handle).unwrap_or_else(|e| {
                    eprintln!("[sync] ratings unavailable: {e}");
                    Vec::new()
                });
                // Submissions failing must not be silent: without them the
                // solved map cannot advance, so surface it as a hard error.
                let subs = client.fetch_submissions(&handle, 2000)?;
                Ok(SyncOutcome::Done { profile, ratings, subs })
            })();
            let _ = tx.send(match outcome {
                Ok(o) => o,
                Err(e) => SyncOutcome::Failed(e),
            });
        });
    }

    fn apply_sync_outcome(&mut self, out: SyncOutcome) {
        fn sync_trace(msg: String) {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                .open(std::env::temp_dir().join("peng-sync-trace.log"))
            {
                use std::io::Write;
                let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
            }
        }
        sync_trace(format!("outcome arrived, syncing flag={}", self.syncing));
        self.syncing = false;
        match out {
            SyncOutcome::Done { profile, ratings, subs } => {
                let handle = self.store.data.settings.handle.clone();
                let n =
                    self.store.apply_sync(&handle, profile, ratings, &subs, self.now_ts, self.utc_off);
                sync_trace(format!(
                    "merged {n} new; solved={} subs={} saving...",
                    self.store.data.solved.len(),
                    self.store.data.submissions.len()
                ));
                match self.store.save() {
                    Ok(()) => sync_trace("save ok".into()),
                    Err(e) => {
                        sync_trace(format!("save FAILED: {e}"));
                        self.toast(format!("Save failed: {e}"), theme::BAD);
                    }
                }
                self.dirty = true;
                self.toast(format!("Synced · {n} new solved"), theme::GOOD);
            }
            SyncOutcome::Failed(e) => {
                self.toast(format!("Sync failed: {e}"), theme::BAD);
            }
        }
    }

    /// One-shot session login on a worker thread. The password is moved into
    /// the thread, used for a single POST, then dropped — never persisted.
    pub fn trigger_login(&mut self) {
        if self.authing {
            return;
        }
        let handle = self.login_handle_buf.trim().to_string();
        let password = std::mem::take(&mut self.login_pass_buf);
        if handle.is_empty() || password.is_empty() {
            self.login_pass_buf = password;
            self.toast("Enter handle/email and password", theme::WARN);
            return;
        }
        self.authing = true;
        let tx = self.auth_tx.clone();
        std::thread::spawn(move || {
            let out = match crate::cf::WebSession::login(&handle, &password) {
                Ok(session) => AuthOutcome::LoggedIn { session, handle: handle.clone() },
                Err(e) => AuthOutcome::Failed(e),
            };
            let _ = tx.send(out);
        });
    }

    fn poll_auth(&mut self) {
        while let Ok(out) = self.auth_rx.try_recv() {
            self.authing = false;
            match out {
                AuthOutcome::LoggedIn { session, handle } => {
                    self.cf_session = Some(session);
                    self.login_handle_buf = handle.clone();
                    self.toast(format!("Logged in as {handle} (this session only)"), theme::GOOD);
                }
                AuthOutcome::Failed(e) => {
                    self.toast(format!("Login failed: {e}"), theme::BAD);
                }
            }
        }
    }

    fn poll_submit(&mut self) {
        while let Ok(out) = self.submit_rx.try_recv() {
            match out {
                SubmitOutcome::Sent { at_ts, session } => {
                    self.cf_session = Some(session);
                    self.submit_state = editor::SubmitState::Sent { at_ts };
                    self.toast("Submitted — check the verdict in a few seconds", theme::GOOD);
                }
                SubmitOutcome::SubmitFailed { error, session } => {
                    // A dead session reads exactly like this; say so plainly.
                    self.cf_session = session;
                    self.submit_state = editor::SubmitState::Failed(error.clone());
                    self.toast(format!("Submit failed: {error}"), theme::BAD);
                }
                SubmitOutcome::Verdict { text, ok } => {
                    self.submit_state = editor::SubmitState::Verdict {
                        text: text.clone(),
                        ok,
                    };
                    if ok {
                        // The AC is real — pull it into progress immediately.
                        self.trigger_refresh();
                    } else {
                        self.toast(format!("Verdict: {text}"), theme::WARN);
                    }
                }
                SubmitOutcome::VerdictFailed(e) => {
                    self.toast(format!("Verdict check failed: {e}"), theme::BAD);
                }
            }
        }
    }

    fn poll_sync(&mut self) {
        while let Ok(out) = self.sync_rx.try_recv() {
            self.apply_sync_outcome(out);
        }
        // One gentle reminder per session when today's streak is about to die.
        if self.dash.streak_at_risk && !self.at_risk_toast_done {
            self.at_risk_toast_done = true;
            self.toast(
                format!(
                    "{}-day streak at risk — solve one problem today!",
                    self.dash.streak.current
                ),
                crate::ui::theme::WARN,
            );
        }

        // Stale-sync guard so a hung request can't pin the UI forever.
        if self.syncing && self.now_ts - self.sync_started_at > 90 {
            self.syncing = false;
            self.toast("Sync timed out", theme::WARN);
        }
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.themed {
            theme::install(&ctx);
            self.themed = true;
        }
        self.poll_sync();
        self.poll_auth();
        self.poll_submit();
        if self.dirty {
            self.rebuild_dash();
            self.dirty = false;
        }

        egui::Panel::left("nav")
            .exact_size(192.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(14, 18)),
            )
            .show(ui, |ui| self.nav(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::same(18)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.screen.clone() {
                        Screen::Dashboard => dashboard::show(self, ui),
                        Screen::Programs => programs::show(self, ui),
                        Screen::Problems => problems::show(self, ui),
                        Screen::Settings => settings::show(self, ui),
                        Screen::ProgramDetail(id) => program_detail::show(self, ui, &id),
                        Screen::Editor(key) => editor::show(self, ui, &key),
                    });
            });

        self.paint_toast(&ctx);
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            let c = r.center();
            let p = ui.painter();
            p.circle_stroke(c, 6.5, Stroke::new(2.5_f32, theme::ACCENT));
            // 270° accent arc reads as a progress ring at a glance.
            let pts: Vec<egui::Pos2> = (0..=24)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * 0.72 * k as f32 / 24.0;
                    c + 6.5 * egui::vec2(a.cos(), a.sin())
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(2.5_f32, theme::ACCENT2)));
            ui.label(RichText::new("RTOM").size(21.0).strong().color(theme::TEXT));
        });
        ui.label(RichText::new("personal accountability").size(11.5).color(theme::FAINT));
        ui.add_space(14.0);

        let items = [
            (Screen::Dashboard, "Home"),
            (Screen::Programs, "Programs"),
            (Screen::Problems, "Problems"),
            (Screen::Settings, "Settings"),
        ];
        for (screen, label) in items {
            let sel = self.screen == screen
                || (matches!(self.screen, Screen::ProgramDetail(_)) && screen == Screen::Programs)
                || (matches!(self.screen, Screen::Editor(_)) && screen == Screen::Problems);
            if ui.selectable_label(sel, RichText::new(label).size(15.5)).clicked() {
                self.screen = screen;
            }
            ui.add_space(2.0);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(8.0);
            let dot = if self.syncing {
                theme::WARN
            } else {
                match self.store.data.last_sync {
                    Some(ts) if self.now_ts - ts < 300 => theme::GOOD,
                    Some(_) => theme::DIM,
                    None => theme::FAINT,
                }
            };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(9.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, dot);
                let msg = if self.syncing {
                    "syncing…".to_string()
                } else {
                    match self.store.data.last_sync {
                        Some(ts) => format!("last sync {}", rel_time(ts, self.now_ts)),
                        None => "never synced".into(),
                    }
                };
                ui.label(RichText::new(msg).size(11.5).color(theme::FAINT));
            });
            ui.label(RichText::new(concat!("RTOM v", env!("CARGO_PKG_VERSION"))).size(11.0).color(theme::FAINT));
        });
    }

    fn paint_toast(&mut self, ctx: &Context) {
        if let Some((msg, expires, color)) = self.toast.take() {
            if (self.now_ts as f64) >= expires {
                return;
            }
            self.toast = Some((msg.clone(), expires, color));
            let screen = ctx.input(|i| i.viewport_rect());
            let p = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("toast"),
            ));
            let galley = p.layout(
                msg.clone(),
                FontId::proportional(14.5),
                theme::TEXT,
                screen.width() - 80.0,
            );
            let size = galley.size() + Vec2::new(34.0, 18.0);
            let pos = Align2::CENTER_BOTTOM
                .anchor_size(screen.center_bottom() - Vec2::new(0.0, 26.0), size);
            p.rect_filled(pos, CornerRadius::same(11), theme::CARD_HI);
            p.rect_stroke(
                pos,
                CornerRadius::same(11),
                Stroke::new(1.0_f32, color),
                egui::StrokeKind::Middle,
            );
            p.galley(pos.min + Vec2::new(17.0, 9.0), galley, theme::TEXT);
        }
    }
}

/// Thin wrapper so screens can open URLs without importing platform everywhere.
pub struct Opener;

impl Opener {
    pub fn open(url: &str) {
        platform::open_url(url);
    }
}

/// Linear color mix (t=0 → a, t=1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        a.r() + ((b.r() as f32 - a.r() as f32) * t) as u8,
        a.g() + ((b.g() as f32 - a.g() as f32) * t) as u8,
        a.b() + ((b.b() as f32 - a.b() as f32) * t) as u8,
    )
}

/// Minimal rating sparkline: painted polyline of `points` across `size`.
pub fn sparkline(
    ui: &mut egui::Ui,
    points: &[(i64, i64)],
    size: egui::Vec2,
    color: Color32,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    if points.len() < 2 {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "sync twice to see your curve",
            FontId::proportional(12.5),
            theme::FAINT,
        );
        return;
    }
    let min_ts = points.iter().map(|q| q.0).min().unwrap();
    let max_ts = points.iter().map(|q| q.0).max().unwrap();
    let min_r = points.iter().map(|q| q.1).min().unwrap();
    let max_r = points.iter().map(|q| q.1).max().unwrap();
    let span_ts = (max_ts - min_ts).max(1) as f32;
    let span_r = (max_r - min_r).max(30) as f32;
    let pad = 6.0;
    let w = rect.width() - pad * 2.0;
    let h = rect.height() - pad * 2.0;
    let pts: Vec<egui::Pos2> = points
        .iter()
        .map(|(ts, r)| {
            egui::Pos2::new(
                rect.left() + pad + (ts - min_ts) as f32 / span_ts * w,
                rect.top() + pad + h - (*r - min_r) as f32 / span_r * h,
            )
        })
        .collect();
    // Baseline
    p.add(Shape::line_segment(
        [egui::pos2(rect.left() + pad, rect.bottom() - pad),
         egui::pos2(rect.right() - pad, rect.bottom() - pad)],
        Stroke::new(1.0_f32, theme::STROKE),
    ));
    p.add(Shape::line(pts.clone(), Stroke::new(2.0_f32, color)));
    let last = pts[pts.len() - 1];
    p.circle_filled(last, 3.5, theme::GOOD);
}
