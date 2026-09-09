//! Problems screen — browse sheets of every loaded pack, open problems on Codeforces.

use super::{card, chip, heading, theme::*, PengApp};
use egui::{Color32, RichText};

pub fn show(app: &mut PengApp, ui: &mut egui::Ui) {
    // Collect sheet ratings across packs (sorted, deduped).
    let mut ratings: Vec<i64> = app
        .store
        .data
        .packs
        .iter()
        .flat_map(|p| p.sheets.iter().map(|s| s.rating))
        .collect();
    ratings.sort_unstable();
    ratings.dedup();

    if app.prob_sheet >= ratings.len() {
        app.prob_sheet = ratings.len().saturating_sub(1);
    }
    let active_rating = ratings.get(app.prob_sheet).copied();

    // Rows for the active sheet: (name, key url, rating label, tags, solved?)
    struct Row {
        name: String,
        url: String,
        rating: i64,
        tags: String,
        solved: bool,
        solved_ts: Option<i64>,
    }
    let mut rows: Vec<Row> = Vec::new();
    if let Some(r) = active_rating {
        for pack in &app.store.data.packs {
            if let Some(sheet) = pack.sheets.iter().find(|s| s.rating == r) {
                for prob in &sheet.problems {
                    let solved = app.store.data.solved.get(&prob.key).copied();
                    let (solved, solved_ts) = match solved {
                        Some(info) => (true, Some(info.ts)),
                        None => (false, None),
                    };
                    let q = app.prob_query.to_lowercase();
                    if !q.is_empty() && !prob.name.to_lowercase().contains(&q) {
                        continue;
                    }
                    if app.prob_only_unsolved && solved {
                        continue;
                    }
                    let tags = if prob.tags.len() > 3 {
                        format!("{}, …", prob.tags[..3].join(", "))
                    } else {
                        prob.tags.join(", ")
                    };
                    rows.push(Row {
                        name: prob.name.clone(),
                        url: prob.key.url(),
                        rating: prob.rating,
                        tags,
                        solved,
                        solved_ts,
                    });
                }
            }
        }
    }
    rows.sort_by_key(|r| (!r.solved, r.rating, r.name.clone()));

    heading(ui, "Problems");
    ui.add_space(2.0);

    // Per-sheet solved counts for tab badges.
    let mut per_sheet: Vec<(i64, usize, usize)> = Vec::new();
    for rt in &ratings {
        let mut total = 0usize;
        let mut done = 0usize;
        for pack in &app.store.data.packs {
            if let Some(sheet) = pack.sheets.iter().find(|sh| sh.rating == *rt) {
                for prob in &sheet.problems {
                    total += 1;
                    if app.store.data.solved.contains_key(&prob.key) {
                        done += 1;
                    }
                }
            }
        }
        per_sheet.push((*rt, total, done));
    }

    // Sheet tabs
    egui::ScrollArea::horizontal().id_salt("problems-tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for (i, rt) in ratings.iter().enumerate() {
                let sel = i == app.prob_sheet;
                let (_, tot, dn) = per_sheet.get(i).copied().unwrap_or((*rt, 0, 0));
                let complete = tot > 0 && dn == tot;
                let label = if complete {
                    format!("{rt} · done")
                } else {
                    format!("{rt} · {dn}/{tot}")
                };
                let txt = RichText::new(label)
                    .size(13.0)
                    .strong()
                    .color(if sel {
                        BG
                    } else if complete {
                        GOOD
                    } else {
                        TEXT
                    });
                if ui
                    .add(
                        egui::Button::new(txt)
                            .fill(if sel { ACCENT } else { CARD })
                            .corner_radius(egui::CornerRadius::same(8))
                            .min_size(egui::Vec2::new(64.0, 26.0)),
                    )
                    .clicked()
                {
                    app.prob_sheet = i;
                }
            }
        });
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.prob_query)
                .hint_text("Search…")
                .desired_width(200.0),
        );
        ui.checkbox(&mut app.prob_only_unsolved, "hide solved");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{} shown · {} solved overall",
                    rows.len(),
                    app.store.data.solved.len()
                ))
                .size(12.0)
                .color(FAINT),
            );
        });
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical().id_salt("problems-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if rows.is_empty() {
                card(ui, |ui| {
                    ui.label(RichText::new("Nothing here.").color(DIM));
                });
                return;
            }
            for row in &rows {
                ui.horizontal(|ui| {
                    let dot = if row.solved { GOOD } else { STROKE };
                    let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, dot);
                    // CF convention: name tinted by problem rating.
                    let tier = tier_for(row.rating);
                    let name_color = if row.solved {
                        DIM
                    } else {
                        super::mix(TEXT, Color32::from_rgb(tier[0], tier[1], tier[2]), 0.55)
                    };
                    let mut tip = row.url.clone();
                    if let Some(ts) = row.solved_ts {
                        tip.push_str(&format!("\nsolved {}", crate::engine::fmt_ts_date(ts)));
                    }
                    if ui
                        .button(RichText::new(&row.name).size(14.5).color(name_color))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        super::Opener::open(&row.url);
                    }
                    chip(
                        ui,
                        &row.rating.to_string(),
                        Color32::from_rgb(tier[0], tier[1], tier[2]),
                    );
                    if !row.tags.is_empty() {
                        ui.label(RichText::new(&row.tags).size(11.5).color(FAINT));
                    }
                });
                ui.add_space(1.0);
                ui.separator();
            }
        });
}

fn tier_for(rating: i64) -> [u8; 3] {
    crate::engine::cf_rank(Some(rating)).color
}
