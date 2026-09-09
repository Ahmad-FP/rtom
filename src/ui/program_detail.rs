//! Per-program detail: milestone ledger, per-sheet breakdown, current-week list.

use super::{bar, card, chip, heading, theme::*, PengApp};
use crate::engine::{self, MilestoneState, PrgStatus};
use egui::{Color32, RichText};

pub fn show(app: &mut PengApp, ui: &mut egui::Ui, program_id: &str) {
    let today = crate::model::date_from_ce(app.today_ord);
    let now = app.now_ts;

    // Snapshot everything the page needs.
    struct Page {
        name: String,
        pack_name: String,
        end: chrono::NaiveDate,
        status: PrgStatus,
        pct: f32,
        count: usize,
        total: usize,
        views: Vec<crate::engine::MilestoneView>,
        sheets: Vec<(i64, usize, usize)>,
        week_slice: Vec<(String, String, i64, bool, Option<i64>)>,
        week_label: String,
        week_target: usize,
        week_due: chrono::NaiveDate,
    }

    let page = (|| -> Option<Page> {
        let prog = app.store.data.programs.iter().find(|p| p.id == program_id)?;
        let pack = app.store.pack_by_id(&prog.pack_id)?;
        let ev = engine::evaluate(prog, pack, &app.store.data.solved, today)?;

        // Per-sheet breakdown within program scope.
        let mut sheets: Vec<(i64, usize, usize)> = Vec::new();
        for rating in &prog.sheet_order {
            let mut total = 0usize;
            let mut done = 0usize;
            if let Some(sheet) = pack.sheets.iter().find(|s| s.rating == *rating) {
                for prob in &sheet.problems {
                    total += 1;
                    if app.store.data.solved.contains_key(&prob.key) {
                        done += 1;
                    }
                }
            }
            if total > 0 {
                sheets.push((*rating, total, done));
            }
        }

        // Current milestone's problem slice.
        let scope = pack.scope_problems(&prog.sheet_order);
        let mut week_slice = Vec::new();
        if let Some(idx) = ev.current_idx {
            let v = &ev.views[idx];
            let prev_target = if idx == 0 { 0 } else { ev.views[idx - 1].target };
            let lo = prev_target.min(scope.len());
            let hi = v.target.min(scope.len());
            for prob in &scope[lo..hi] {
                let solved = app.store.data.solved.get(&prob.key).copied();
                week_slice.push((
                    prob.name.clone(),
                    prob.key.url(),
                    prob.rating,
                    solved.is_some(),
                    solved.map(|i| i.ts),
                ));
            }
            week_slice.sort_by_key(|(_, _, _, solved, _)| (!*solved, *solved));
        }

        Some(Page {
            name: prog.name.clone(),
            pack_name: pack.name.clone(),
            end: prog.end_date(),
            status: ev.status.clone(),
            pct: ev.pct,
            count: ev.solved_count,
            total: ev.total,
            views: ev.views.clone(),
            sheets,
            week_slice,
            week_label: ev.current_idx.and_then(|i| ev.views.get(i)).map(|v| v.label.clone()).unwrap_or_default(),
            week_target: ev.current_idx.and_then(|i| ev.views.get(i)).map(|v| v.target).unwrap_or(0),
            week_due: ev.current_idx.and_then(|i| ev.views.get(i)).map(|v| v.due).unwrap_or(today),
        })
    })();

    if ui.button(RichText::new("← All programs").size(13.0).color(ACCENT2)).clicked() {
        app.screen = super::Screen::Programs;
        return;
    }
    ui.add_space(6.0);

    let Some(page) = page else {
        card(ui, |ui| {
            ui.label(RichText::new("Program not found.").color(DIM));
        });
        return;
    };

    heading(ui, &page.name);
    ui.label(
        RichText::new(format!(
            "{} · {}/{} solved · {:.0}% · ends {}",
            page.pack_name,
            page.count,
            page.total,
            page.pct,
            engine::fmt_date(page.end)
        ))
        .size(13.0)
        .color(DIM),
    );
    ui.add_space(8.0);

    // --- Milestone ledger -------------------------------------------------------
    card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new("MILESTONES").size(11.5).color(FAINT));
        ui.add_space(6.0);
        for (vi, v) in page.views.iter().enumerate() {
            let prev_target =
                if vi == 0 { 0 } else { page.views[vi - 1].target };
            ui.horizontal(|ui| {
                let (dot, label_color) = match &v.state {
                    MilestoneState::Done { late: false } => (GOOD, TEXT),
                    MilestoneState::Done { late: true } => (WARN, DIM),
                    MilestoneState::Active { on_track: true, .. } => (ACCENT, TEXT),
                    MilestoneState::Active { .. } => (BAD, TEXT),
                    MilestoneState::Missed { .. } => (BAD, DIM),
                    MilestoneState::Future => (STROKE, FAINT),
                };
                let (r, resp) =
                    ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                resp.on_hover_text(format!("{} · target {}", v.label, v.target));
                ui.label(RichText::new(&v.label).size(14.0).color(label_color));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let note = match (&v.state, v.now.min(v.target)) {
                        (MilestoneState::Done { late: false }, _) => "done".to_string(),
                        (MilestoneState::Done { late: true }, _) => "done (late)".to_string(),
                        (MilestoneState::Missed { deficit }, _) => {
                            format!("missed · {deficit} short")
                        }
                        (MilestoneState::Active { on_track: true, .. }, n) => {
                            format!("{n} / {} · on track", v.target)
                        }
                        (MilestoneState::Active { deficit, .. }, n) => {
                            format!("{n} / {} · {} behind", v.target, deficit)
                        }
                        (MilestoneState::Future, _) => format!(
                            "+{} · unlocks {}",
                            v.target.saturating_sub(prev_target),
                            crate::engine::fmt_date(v.due)
                        ),
                    };
                    ui.label(RichText::new(note).size(12.5).color(DIM));
                });
            });
            bar(ui, v.now as f32 / v.target.max(1) as f32, ui.available_width() - 8.0, 5.0, {
                match &v.state {
                    MilestoneState::Done { .. } => GOOD,
                    MilestoneState::Active { on_track: true, .. } => ACCENT,
                    _ => BAD,
                }
            });
            ui.add_space(7.0);
        }
    });

    // --- Per-sheet breakdown ------------------------------------------------------
    ui.add_space(10.0);
    card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new("SHEET BREAKDOWN").size(11.5).color(FAINT));
        ui.add_space(6.0);
        for (rating, total, done) in &page.sheets {
            ui.horizontal(|ui| {
                let t = crate::engine::cf_rank(Some(*rating));
                chip(
                    ui,
                    &format!("{rating}"),
                    Color32::from_rgb(t.color[0], t.color[1], t.color[2]),
                );
                let frac = *done as f32 / (*total).max(1) as f32;
                let col = if done == total {
                    GOOD
                } else if *done > 0 {
                    ACCENT
                } else {
                    STROKE
                };
                bar(ui, frac, ui.available_width() - 150.0, 6.0, col);
                ui.label(
                    RichText::new(format!("{done}/{total}"))
                        .size(12.0)
                        .color(if done == total { GOOD } else { DIM }),
                );
            });
            ui.add_space(3.0);
        }
    });

    // --- Current week problems -----------------------------------------------------
    ui.add_space(10.0);
    card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("THIS WEEK'S PROBLEMS").size(11.5).color(FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "target {} · due {}",
                        page.week_target,
                        engine::fmt_date(page.week_due)
                    ))
                    .size(12.0)
                    .color(FAINT),
                );
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("detail-week")
            .max_height(300.0)
            .show(ui, |ui| {
                if page.week_slice.is_empty() {
                    ui.label(RichText::new("Nothing scheduled.").color(DIM));
                    return;
                }
                for (name, url, rating, solved, ts) in &page.week_slice {
                    ui.horizontal(|ui| {
                        let dot = if *solved { GOOD } else { STROKE };
                        let (r, _) = ui
                            .allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, dot);
                        let mut tip = url.clone();
                        if let Some(ts) = ts {
                            tip.push_str(&format!("\nsolved {}", engine::fmt_ts_date(*ts)));
                        }
                        if ui
                            .button(
                                RichText::new(name)
                                    .size(14.0)
                                    .color(if *solved { DIM } else { TEXT }),
                            )
                            .on_hover_text(tip)
                            .clicked()
                        {
                            super::Opener::open(url);
                        }
                        chip(ui, &rating.to_string(), {
                            let t = engine::cf_rank(Some(*rating)).color;
                            Color32::from_rgb(t[0], t[1], t[2])
                        });
                    });
                    ui.add_space(1.0);
                }
            });
    });

    let _ = (now, &page.status, &page.week_label);
}
