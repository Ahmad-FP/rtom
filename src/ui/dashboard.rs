//! Dashboard — the screen you land on: current goal, streak, rank, sync,
//! activity calendar and rating trend.

use super::{bar, card, chip, flame_chip, heading, pill, rel_time, ring, sparkline, stat_card, theme, PengApp};
use crate::engine::{self, MilestoneState, PrgStatus};
use egui::RichText;
use std::cell::Cell;

pub fn show(app: &mut PengApp, ui: &mut egui::Ui) {
    let dash = app.dash.clone();
    let prog = dash.progress.clone();
    let handle = app.store.data.settings.handle.clone();
    let last_sync = app.store.data.last_sync;
    let cf_rating = app.store.data.profile.as_ref().and_then(|p| p.rating);
    let syncing = app.syncing;
    let now = app.now_ts;
    let off = app.utc_off;
    let today_o = app.today_ord;

    /// 1 → "1st", 2 → "2nd", 3 → "3rd", 4/11/12 → "th"…
    fn ordinal(n: u32) -> String {
        let suffix = if (11..=13).contains(&(n % 100)) {
            "th"
        } else {
            match n % 10 {
                1 => "st",
                2 => "nd",
                3 => "rd",
                _ => "th",
            }
        };
        format!("{n}{suffix}")
    }

    heading(ui, "Home");

    // --- Header -----------------------------------------------------------------
    let hour = ((now + off as i64 * 60).rem_euclid(86_400) / 3_600) as u32;
    let greet = if (5..12).contains(&hour) {
        "Good morning"
    } else if (12..17).contains(&hour) {
        "Good afternoon"
    } else {
        "Good evening"
    };
    let who = if handle.is_empty() { String::new() } else { format!(", {handle}") };
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{greet}{who}")).size(15.0).color(theme::DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tier = engine::cf_rank(cf_rating);
            match cf_rating {
                Some(r) => chip(
                    ui,
                    &format!("{r} · {}", tier.name),
                    egui::Color32::from_rgb(tier.color[0], tier.color[1], tier.color[2]),
                ),
                None => chip(ui, "no CF data yet", theme::FAINT),
            }
            ui.horizontal(|ui| {
                let (into, width) = (dash.rank_into, dash.rank_width);
                bar(ui, into as f32 / width.max(1) as f32, 64.0, 5.0, theme::ACCENT);
                ui.label(RichText::new(format!("{into}/{width}")).size(11.0).color(theme::FAINT));
            });
            chip(ui, dash.rank_name, rank_color(dash.rank_name));
            if dash.streak.current > 0 {
                flame_chip(ui, dash.streak.current);
            } else {
                chip(ui, "start a streak today", theme::WARN);
            }
        });
    });
    ui.add_space(8.0);

    // --- Hero: current weekly goal --------------------------------------------
    let refresh_clicked = Cell::new(false);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let (frac, label) = match &prog {
                Some(p) => (p.pct / 100.0, format!("{:.0}%", p.pct)),
                None => (0.0, "—".into()),
            };
            let color = match prog.as_ref().map(|p| p.status) {
                Some(PrgStatus::Behind) => theme::BAD,
                Some(PrgStatus::Ahead) => theme::ACCENT2,
                Some(PrgStatus::Complete) => theme::GOOD,
                _ => theme::ACCENT,
            };
            ring(ui, 138.0, frac, color, label, "overall".into());

            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width());
                match (&prog, prog.as_ref().and_then(|p| p.current_idx)) {
                    (Some(p), Some(idx)) if idx < p.views.len() => {
                        let v = &p.views[idx];
                        let status = match (&v.state, p.status) {
                            (MilestoneState::Missed { .. }, _) => ("behind schedule", theme::BAD),
                            (_, PrgStatus::Behind) => ("behind schedule", theme::BAD),
                            (_, PrgStatus::Ahead) => ("ahead", theme::ACCENT2),
                            (_, PrgStatus::Complete) => ("complete!", theme::GOOD),
                            _ => ("on track", theme::GOOD),
                        };
                        ui.label(RichText::new("CURRENT GOAL").size(11.0).color(theme::FAINT));
                        ui.label(RichText::new(&dash.program_name).size(19.0).strong());
                        if let Some(started) = &dash.program_start {
                            let elapsed = (today_o
                                - chrono::Datelike::num_days_from_ce(started))
                            .max(0);
                            ui.label(
                                RichText::new(format!(
                                    "{} from the day you began, the {} of {}, {}",
                                    if elapsed == 1 { "1 day".into() } else { format!("{elapsed} days") },
                                    ordinal(chrono::Datelike::day(started)),
                                    started.format("%B"),
                                    chrono::Datelike::year(started)
                                ))
                                .size(12.5)
                                .color(theme::FAINT),
                            );
                        }
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(&v.label).size(14.0).color(theme::ACCENT2),
                                    )
                                    .fill(theme::CARD)
                                    .corner_radius(egui::CornerRadius::same(7)),
                                )
                                .on_hover_text("Show this week's problem list")
                                .clicked()
                            {
                                app.goal_detail_open = true;
                            }
                            pill(ui, status.0, status.1);
                        });
                        ui.add_space(4.0);
                        let deficit_note = match v.state.clone() {
                            MilestoneState::Active { deficit, .. } if deficit > 0 => {
                                format!(" · {} behind pace", deficit)
                            }
                            _ => String::new(),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} / {} problems cumulative{}",
                                v.now.min(v.target),
                                v.target,
                                deficit_note
                            ))
                            .size(13.5)
                            .color(theme::DIM),
                        );
                        ui.add_space(6.0);
                        bar(
                            ui,
                            v.now as f32 / v.target.max(1) as f32,
                            ui.available_width(),
                            8.0,
                            color,
                        );
                    }
                    _ => {
                        ui.label(RichText::new("No active goal").size(19.0).strong());
                        ui.label(
                            RichText::new("Create a training program to get going.")
                                .color(theme::DIM),
                        );
                    }
                }
                if let Some(p) = &prog {
                    if let Some(v) = p.views.get(p.current_idx.unwrap_or(0)) {
                        let due_ord = chrono::Datelike::num_days_from_ce(&v.due);
                        let due_ts = engine::day_start_ts(due_ord, off) + 86_400 - 1;
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!(
                                "due in {} · {}",
                                engine::fmt_duration((due_ts - now).max(0)),
                                engine::fmt_date(v.due)
                            ))
                            .size(13.0)
                            .color(theme::WARN),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let btn = egui::Button::new(
                        RichText::new(if syncing { "  Syncing…  " } else { "⟳ Refresh Codeforces" })
                            .size(14.5)
                            .color(theme::BG),
                    )
                    .fill(if syncing { theme::STROKE } else { theme::ACCENT })
                    .corner_radius(egui::CornerRadius::same(9))
                    .min_size(egui::Vec2::new(180.0, 34.0));
                    if ui.add_enabled(!syncing, btn).clicked() {
                        refresh_clicked.set(true);
                    }
                    if let Some(url) = &dash.next_up_url {
                        if ui.button(RichText::new("Next problem ↗").size(14.0)).clicked() {
                            let url = url.clone();
                            super::Opener::open(&url);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ts) = last_sync {
                            ui.label(
                                RichText::new(format!("synced {}", rel_time(ts, now)))
                                    .size(12.0)
                                    .color(theme::FAINT),
                            );
                        }
                    });
                });
            });
        });
    });

    if refresh_clicked.get() {
        app.trigger_refresh();
    }

    // --- Streak-at-risk banner ----------------------------------------------------
    if dash.streak_at_risk {
        let go = std::cell::Cell::new(false);
        card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(26.0), egui::Sense::hover());
                let c = r.center();
                let s = 11.0f32;
                let outer = [
                    c + s * egui::vec2(-0.55, 0.90),
                    c + s * egui::vec2(0.55, 0.90),
                    c + s * egui::vec2(0.75, -0.10),
                    c + s * egui::vec2(0.00, -1.10),
                ];
                ui.painter().add(egui::Shape::convex_polygon(
                    outer.to_vec(),
                    theme::WARN,
                    egui::Stroke::NONE,
                ));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!("{}-day streak at risk", dash.streak.current))
                            .size(16.0)
                            .strong()
                            .color(theme::WARN),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Solve one problem before midnight — {} left on the clock.",
                            engine::fmt_duration(dash.secs_to_midnight.max(0))
                        ))
                        .size(13.0)
                        .color(theme::DIM),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Pick a problem").color(theme::BG))
                                .fill(theme::WARN)
                                .corner_radius(egui::CornerRadius::same(9)),
                        )
                        .clicked()
                    {
                        go.set(true);
                    }
                });
            });
        });
        if go.get() {
            app.screen = super::Screen::Problems;
        }
        ui.add_space(10.0);
    }

    ui.add_space(10.0);

    // --- Stats row --------------------------------------------------------------
    ui.columns(4, |cols| {
        stat_card(&mut cols[0], "solved today", dash.solved_today.to_string(), theme::GOOD);
        stat_card(&mut cols[1], "this week", dash.week_solves.to_string(), theme::ACCENT2);
        stat_card(&mut cols[2], "total solved", dash.solved_total.to_string(), theme::TEXT);
        stat_card(&mut cols[3], "longest streak", format!("{}d", dash.streak.longest), theme::FLAME);
    });

    // --- Activity: 4-week solve calendar -------------------------------------------
    {
        use chrono::Datelike;
        let mut per_day: std::collections::BTreeMap<i32, usize> =
            std::collections::BTreeMap::new();
        for info in app.store.data.solved.values() {
            *per_day.entry(info.day).or_default() += 1;
        }
        let today_o = app.today_ord;
        let today_date = crate::model::date_from_ce(today_o);
        let dow_today = today_date.weekday().num_days_from_monday() as i32;
        let weeks = 4i32;
        let start = today_o - (weeks * 7 - 1) - (6 - dow_today);

        ui.add_space(10.0);
        card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new("ACTIVITY · LAST 4 WEEKS").size(11.0).color(theme::FAINT),
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for l in ["M", "T", "W", "T", "F", "S", "S"] {
                        ui.set_min_width(16.0);
                        ui.label(RichText::new(l).size(9.5).color(theme::FAINT));
                    }
                });
                for w in 0..weeks {
                    ui.horizontal(|ui| {
                        for dow in 0..7i32 {
                            let day = start + w * 7 + dow;
                            if day > today_o {
                                let _ = ui.allocate_exact_size(
                                    egui::Vec2::splat(15.0),
                                    egui::Sense::hover(),
                                );
                                continue;
                            }
                            let n = per_day.get(&day).copied().unwrap_or(0);
                            let is_today = day == today_o;
                            let (r, resp) = ui.allocate_exact_size(
                                egui::Vec2::splat(15.0),
                                egui::Sense::hover(),
                            );
                            let p = ui.painter_at(r);
                            p.rect_filled(
                                r.shrink(1.5),
                                egui::CornerRadius::same(3),
                                theme::CARD_HI,
                            );
                            if n > 0 {
                                let col = match n {
                                    1 => theme::GOOD.gamma_multiply(0.5),
                                    2 => theme::GOOD.gamma_multiply(0.75),
                                    _ => theme::GOOD,
                                };
                                p.rect_filled(r.shrink(2.5), egui::CornerRadius::same(2), col);
                            }
                            if is_today {
                                p.rect_stroke(
                                    r,
                                    egui::CornerRadius::same(3),
                                    egui::Stroke::new(1.2, theme::ACCENT),
                                    egui::StrokeKind::Middle,
                                );
                            }
                            let date = crate::model::date_from_ce(day);
                            resp.on_hover_text(format!("{} · {} solved", date.format("%b %d"), n));
                        }
                    });
                }
            });
        });

    }

    // --- Milestone timeline -------------------------------------------------------
    if let Some(p) = &prog {
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.label(RichText::new("MILESTONE TIMELINE").size(11.0).color(theme::FAINT));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let spacing = 34.0;
                let dot_r = 5.5f32;
                for (i, v) in p.views.iter().enumerate() {
                    if i > 0 {
                        let prev_done =
                            matches!(p.views[i - 1].state, MilestoneState::Done { .. });
                        let line_color = if prev_done {
                            theme::GOOD.gamma_multiply(0.7)
                        } else {
                            theme::STROKE
                        };
                        let (rect, _) = ui.allocate_exact_size(
                            egui::Vec2::new(spacing - dot_r * 2.0, 2.0),
                            egui::Sense::hover(),
                        );
                        ui.painter_at(rect).rect_filled(rect, 1.0, line_color);
                    }
                    let color = match &v.state {
                        MilestoneState::Done { late: false } => theme::GOOD,
                        MilestoneState::Done { late: true } => theme::WARN,
                        MilestoneState::Active { on_track: true, .. } => theme::ACCENT,
                        MilestoneState::Active { .. } => theme::BAD,
                        MilestoneState::Missed { .. } => theme::BAD,
                        MilestoneState::Future => theme::STROKE,
                    };
                    let (r, resp) = ui.allocate_exact_size(
                        egui::Vec2::splat(dot_r * 2.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().circle_filled(r.center(), dot_r, color);
                    resp.on_hover_text(format!(
                        "{}\ntarget {} problems ({:.0}%)\ndue {}",
                        v.label,
                        v.target,
                        v.cum_pct,
                        v.due.format("%b %d")
                    ));
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for v in &p.views {
                    let done = matches!(v.state, MilestoneState::Done { .. });
                    ui.monospace(
                        RichText::new(format!("W{}", v.idx + 1))
                            .size(11.0)
                            .color(if done { theme::GOOD } else { theme::FAINT }),
                    );
                    if v.idx + 1 < p.views.len() {
                        ui.add_space(23.0);
                    }
                }
            });
        });
    }

    // --- First-run CTA ----------------------------------------------------------
    if handle.is_empty() && last_sync.is_none() {
        ui.add_space(10.0);
        let go = std::cell::Cell::new(false);
        card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Connect your Codeforces account").size(17.0).strong());
            ui.label(
                RichText::new(
                    "RTOM tracks your ACs, streaks and rating — but it only ever \
                     fetches when you ask. Add your handle to start the loop.",
                )
                .size(13.5)
                .color(theme::DIM),
            );
            ui.add_space(4.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("Add handle").color(theme::BG))
                        .fill(theme::ACCENT)
                        .corner_radius(egui::CornerRadius::same(9)),
                )
                .clicked()
            {
                go.set(true);
            }
        });
        if go.get() {
            app.screen = super::Screen::Settings;
        }
    }

    // --- Rating trend ------------------------------------------------------------
    ui.add_space(10.0);
    let ratings: Vec<(i64, i64)> = app
        .store
        .data
        .rating_history
        .iter()
        .map(|r| (r.ts, r.rating))
        .collect();
    let latest = ratings.last().map(|(_, r)| *r);
    card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("RATING TREND").size(11.0).color(theme::FAINT));
            if let Some(r) = latest {
                let tier = engine::cf_rank(Some(r));
                chip(
                    ui,
                    &format!("{r} · {}", tier.name),
                    egui::Color32::from_rgb(tier.color[0], tier.color[1], tier.color[2]),
                );
            }
        });
        sparkline(
            ui,
            &ratings,
            egui::Vec2::new(ui.available_width(), 84.0),
            theme::ACCENT2,
        );
    });
}

fn rank_color(name: &str) -> egui::Color32 {
    let t = engine::cf_rank(Some(match name {
        "pupil" => 1300,
        "specialist" => 1500,
        "expert" => 1700,
        "candidate master" => 2000,
        _ => 900,
    }));
    egui::Color32::from_rgb(t.color[0], t.color[1], t.color[2])
}
