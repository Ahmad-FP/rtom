//! Programs screen — list training programs, create manual ones with
//! per-percentage deadlines (milestones), delete with confirmation.

use super::{bar, card, heading, pill, theme::*, PengApp};
use crate::engine::{self, PrgStatus};
use crate::model::{Milestone, Program};
use egui::{Align, Color32, Layout, RichText};

#[derive(Clone)]
pub struct Draft {
    pub name: String,
    pub pack_idx: usize,
    pub weeks: f32,
    pub custom: bool,
    /// (percent, due day) rows for custom mode.
    pub rows: Vec<(String, String)>,
    pub err: String,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            name: String::new(),
            pack_idx: 0,
            weeks: 8.0,
            custom: false,
            rows: vec![("50".into(), "14".into()), ("100".into(), "28".into())],
            err: String::new(),
        }
    }
}

pub fn show(app: &mut PengApp, ui: &mut egui::Ui) {
    let today = crate::model::date_from_ce(app.today_ord);
    let now = app.now_ts;

    // Snapshot program cards.
    let cards: Vec<(String, String, f32, usize, usize, Option<chrono::NaiveDate>, PrgStatus)> =
        app.store
            .data
            .programs
            .iter()
            .map(|p| {
                let pack = app.store.pack_by_id(&p.pack_id);
                let ev = pack
                    .and_then(|pk| engine::evaluate(p, pk, &app.store.data.solved, today));
                (
                    p.id.clone(),
                    p.name.clone(),
                    ev.as_ref().map(|e| e.pct).unwrap_or(0.0),
                    ev.as_ref().map(|e| e.solved_count).unwrap_or(0),
                    ev.as_ref().map(|e| e.total).unwrap_or(0),
                    Some(p.end_date()),
                    ev.map(|e| e.status).unwrap_or(PrgStatus::OnTrack),
                )
            })
            .collect();
    let packs = app.store.data.packs.clone();

    heading(ui, "Programs");

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Existing programs
            for (id, name, pct, count, total, end, status) in &cards {
                let confirm = app.prog_confirm_del.as_deref() == Some(id.as_str());
                card(ui, |ui| {
                    if std::env::var("PENG_DBGW").is_ok() {
                        ui.label(RichText::new(format!(
                            "DBG inner_w={:.0} avail={:.0} min={:.0}",
                            ui.min_rect().width(),
                            ui.available_width(),
                            ui.min_size().x
                        )).size(11.0).color(ACCENT2));
                    }
                    // Title row: name (or rename editor) + pill + actions.
                    ui.horizontal_wrapped(|ui| {
                        let renaming = app
                            .prog_rename
                            .as_ref()
                            .map(|(rid, _)| rid == id)
                            .unwrap_or(false);
                        if renaming {
                            let mut buf = app.prog_rename.clone().unwrap().1;
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut buf).desired_width(220.0),
                            );
                            let save =
                                ui.button(RichText::new("Save").size(12.5).color(GOOD)).clicked();
                            let enter = resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let cancel =
                                ui.button(RichText::new("Cancel").size(12.5).color(DIM)).clicked();
                            if save || enter {
                                let new_name = buf.trim().to_string();
                                if !new_name.is_empty() {
                                    if let Some(prog) =
                                        app.store.data.programs.iter_mut().find(|p| &p.id == id)
                                    {
                                        prog.name = new_name;
                                    }
                                    let _ = app.store.save();
                                    app.mark_dirty();
                                    app.toast("Program renamed", GOOD);
                                }
                                app.prog_rename = None;
                            } else if cancel {
                                app.prog_rename = None;
                            } else {
                                app.prog_rename = Some((id.clone(), buf));
                            }
                        } else {
                            let open = ui
                                .add(
                                    egui::Button::new(RichText::new(name).size(16.5).strong())
                                        .fill(Color32::TRANSPARENT),
                                )
                                .on_hover_text("Open program details")
                                .clicked();
                            ui.add_space(4.0);
                            pill(
                                ui,
                                match status {
                                    PrgStatus::Complete => "complete",
                                    PrgStatus::Ahead => "ahead",
                                    PrgStatus::Behind => "behind",
                                    PrgStatus::OnTrack => "on track",
                                },
                                match status {
                                    PrgStatus::Complete => GOOD,
                                    PrgStatus::Ahead => ACCENT2,
                                    PrgStatus::Behind => BAD,
                                    PrgStatus::OnTrack => GOOD,
                                },
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let del = if confirm { "sure?" } else { "×" };
                                if ui
                                    .button(
                                        RichText::new(del)
                                            .size(12.5)
                                            .color(if confirm { BAD } else { DIM }),
                                    )
                                    .clicked()
                                {
                                    if confirm {
                                        app.prog_confirm_del = None;
                                        app.store.data.programs.retain(|p| &p.id != id);
                                        let _ = app.store.save();
                                        app.mark_dirty();
                                        app.toast("Program deleted", WARN);
                                    } else {
                                        app.prog_confirm_del = Some(id.clone());
                                    }
                                }
                                if confirm && ui.button("keep").clicked() {
                                    app.prog_confirm_del = None;
                                }
                                if !confirm
                                    && ui
                                        .button(RichText::new("Copy").size(12.5).color(DIM))
                                        .clicked()
                                {
                                    if let Some(orig) = app
                                        .store
                                        .data
                                        .programs
                                        .iter()
                                        .find(|p| &p.id == id)
                                        .cloned()
                                    {
                                        let now = app.now_ts;
                                        let mut dup = orig.clone();
                                        dup.id = format!("prog-{}-copy", now);
                                        dup.name = format!("{} (copy)", orig.name);
                                        // A copy is a fresh attempt: clock restarts today.
                                        dup.start_day =
                                            crate::engine::day_ord_of_ts(now, app.utc_off);
                                        dup.created_at = now;
                                        app.store.data.programs.push(dup);
                                        let _ = app.store.save();
                                        app.mark_dirty();
                                        app.toast("Program duplicated — clock restarted", GOOD);
                                    }
                                }
                                if !confirm
                                    && ui
                                        .button(RichText::new("Rename").size(12.5).color(DIM))
                                        .clicked()
                                {
                                    app.prog_rename = Some((id.clone(), name.clone()));
                                }
                            });
                            if open {
                                app.screen = crate::ui::Screen::ProgramDetail(id.clone());
                            }
                        }
                    });
                    bar(ui, *pct / 100.0, ui.available_width() - 8.0, 7.0, ACCENT);
                    ui.label(
                        RichText::new(format!(
                            "{count} / {total} solved · {:.0}% · ends {}",
                            pct,
                            end.map(engine::fmt_date).unwrap_or_default()
                        ))
                        .size(12.5)
                        .color(DIM),
                    );
                });
                ui.add_space(2.0);
            }

            // New-program card / wizard
            if !app.wizard_open {
                if ui
                   .add(egui::Button::new(
                        RichText::new("+ New training program").size(15.0).color(ACCENT),
                    ))
                    .clicked()
                {
                    app.wizard_open = true;
                    app.draft = Draft::default();
                }
            } else {
                wizard(app, ui, &packs, now);
            }
        });
}

fn wizard(app: &mut PengApp, ui: &mut egui::Ui, packs: &[crate::model::Pack], now: i64) {
    let mut draft = app.draft.clone();
    let mut create = false;
    let mut cancel = false;

    card(ui, |ui| {
        ui.label(RichText::new("NEW TRAINING PROGRAM").size(11.5).color(FAINT));
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").color(DIM));
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("e.g. CP-31 sprint")
                    .desired_width(240.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Problem set").color(DIM));
            egui::ComboBox::from_id_salt("pack-select")
                .selected_text(
                    packs
                        .get(draft.pack_idx)
                        .map(|p| p.name.as_str())
                        .unwrap_or("—"),
                )
                .show_ui(ui, |ui| {
                    for (i, p) in packs.iter().enumerate() {
                        ui.selectable_value(&mut draft.pack_idx, i, &p.name);
                    }
                });
            ui.checkbox(&mut draft.custom, "custom milestones");
        });

        if !draft.custom {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Duration").color(DIM));
                ui.add(egui::Slider::new(&mut draft.weeks, 1.0..=26.0).suffix(" weeks"));
                ui.label(
                    RichText::new(format!(
                        "auto: {} weekly milestones of {:.1}% each",
                        draft.weeks as i32,
                        100.0 / draft.weeks
                    ))
                    .size(12.0)
                    .color(FAINT),
                );
            });
        } else {
            ui.label(RichText::new("Milestones — cumulative % by day offset").size(13.0).color(DIM));
            let mut add_row = false;
            let mut remove: Option<usize> = None;
            for (i, (pct, day)) in draft.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(pct)
                            .desired_width(56.0)
                            .hint_text("%"),
                    );
                    ui.label(RichText::new("% by").color(DIM));
                    ui.add(egui::TextEdit::singleline(day).desired_width(56.0).hint_text("day"));
                    if ui.button("×").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if ui.button("+ milestone").clicked() {
                add_row = true;
            }
            if let Some(i) = remove {
                draft.rows.remove(i);
            }
            if add_row {
                let last_day = draft.rows.last().and_then(|(_, d)| d.parse::<i32>().ok());
                draft.rows.push(("100".into(), format!("{}", last_day.unwrap_or(28) + 7)));
            }
        }

        if !draft.err.is_empty() {
            ui.label(RichText::new(&draft.err).size(13.0).color(BAD));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("Create").color(BG)).fill(ACCENT))
                .clicked()
            {
                create = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });

    if cancel {
        app.wizard_open = false;
        return;
    }
    if create {
        match build_program(&draft, packs, now, app.utc_off) {
            Ok(prog) => {
                app.store.data.programs.push(prog);
                let _ = app.store.save();
                app.mark_dirty();
                app.toast("Training program created", GOOD);
                app.wizard_open = false;
            }
            Err(e) => draft.err = e,
        }
    }
    app.draft = draft;
}

fn build_program(
    draft: &Draft,
    packs: &[crate::model::Pack],
    now: i64,
    off: i32,
) -> Result<Program, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the program a name.".into());
    }
    let Some(pack) = packs.get(draft.pack_idx) else {
        return Err("Pick a problem set.".into());
    };

    let mut milestones: Vec<Milestone> = Vec::new();
    if !draft.custom {
        let w = draft.weeks.round().max(1.0) as i32;
        for k in 1..=w {
            milestones.push(Milestone {
                label: format!("Week {k}"),
                due_day: 7 * k,
                cum_pct: 100.0 * k as f32 / w as f32,
            });
        }
    } else {
        for (i, (pct_s, day_s)) in draft.rows.iter().enumerate() {
            let pct: f32 = pct_s.trim().parse().map_err(|_| format!("Row {}: bad %", i + 1))?;
            let day: i32 = day_s.trim().parse().map_err(|_| format!("Row {}: bad day", i + 1))?;
            if !(0.0..=100.0).contains(&pct) {
                return Err(format!("Row {}: % must be 0–100", i + 1));
            }
            if day < 1 {
                return Err(format!("Row {}: day must be ≥ 1", i + 1));
            }
            milestones.push(Milestone {
                label: format!("{:.0}% by day {day}", pct),
                due_day: day,
                cum_pct: pct,
            });
        }
        milestones.sort_by_key(|m| m.due_day);
    }

    if milestones.is_empty() {
        return Err("Add at least one milestone.".into());
    }
    if let Some(last) = milestones.last() {
        if last.cum_pct < 99.9 {
            return Err("Last milestone should reach 100%.".into());
        }
    }

    let order: Vec<i64> = {
        let mut o: Vec<i64> = pack.sheets.iter().map(|s| s.rating).collect();
        o.sort_unstable();
        o
    };
    Ok(Program {
        id: format!("prog-{}", now),
        name: name.to_string(),
        pack_id: pack.id.clone(),
        sheet_order: order,
        start_day: crate::engine::day_ord_of_ts(now, off),
        duration_days: milestones.last().map(|m| m.due_day).unwrap_or(28),
        milestones,
        created_at: now,
    })
}
