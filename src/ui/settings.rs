//! Settings — handle, sync behavior, background/tray options, data management.

use super::{card, heading, theme::*, toggle, PengApp};
use crate::platform;
use egui::{RichText, TextEdit};

pub fn show(app: &mut PengApp, ui: &mut egui::Ui) {
    heading(ui, "Settings");
    ui.add_space(2.0);

    // --- Codeforces -----------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("CODEFORCES").size(11.5).color(FAINT));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Handle").color(DIM));
            let resp = ui
                .add(TextEdit::singleline(&mut app.handle_buf).desired_width(220.0))
                .on_hover_text("Enter your Codeforces handle");
            let save_clicked = ui
                .add(egui::Button::new(RichText::new("Save & refresh").color(BG)).fill(ACCENT))
                .clicked();
            let enter_pressed =
                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if save_clicked || enter_pressed {
                app.store.data.settings.handle = app.handle_buf.trim().to_string();
                let _ = app.store.save();
                app.trigger_refresh();
            }
        });
        ui.label(
            RichText::new(
                "RTOM never polls — it fetches your ACs only when you hit refresh.",
            )
            .size(12.0)
            .color(FAINT),
        );
    });

    // --- Behavior ---------------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("BEHAVIOR").size(11.5).color(FAINT));
        ui.add_space(6.0);

        let mut refresh_on_open = app.store.data.settings.refresh_on_open;
        if setting_row(ui, "Refresh when app opens", "One fetch on launch — still no polling", &mut refresh_on_open) {
            app.store.data.settings.refresh_on_open = refresh_on_open;
        }

        let mut start_minimized = app.store.data.settings.start_minimized;
        if setting_row(ui, "Start minimized to tray", "Launches quietly into the background", &mut start_minimized) {
            app.store.data.settings.start_minimized = start_minimized;
        }

        let mut login = platform::autostart::enabled();
        if setting_row(ui, "Launch at login", "Registers peng with your OS autostart", &mut login) {
            platform::autostart::set(login);
            app.store.data.settings.launch_on_login = login;
        }

        if app.store.data.settings.refresh_on_open
            || app.store.data.settings.start_minimized
            || app.store.data.settings.launch_on_login != platform::autostart::enabled()
        {
            let _ = app.store.save();
        }
    });

    // --- Data --------------------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("DATA").size(11.5).color(FAINT));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Export progress CSV").clicked() {
                let dir = platform::paths::data_dir();
                let path = dir.join(format!(
                    "peng-progress-{}.csv",
                    chrono::Utc::now().format("%Y%m%d-%H%M%S")
                ));
                match export_progress_csv(&app.store.data, &path) {
                    Ok(n) => app.toast(format!("Exported {n} rows"), GOOD),
                    Err(e) => app.toast(format!("Export failed: {e}"), BAD),
                }
            }
            if ui.button("Open data folder").clicked() {
                let dir = platform::paths::data_dir();
                platform::open_url(&dir.to_string_lossy());
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Import a problem pack (JSON)").size(13.5).color(DIM));
        ui.horizontal(|ui| {
            let path_edit =
                ui.add(TextEdit::singleline(&mut app.import_buf).desired_width(320.0).hint_text("path to pack.json"));
            let import_clicked = ui.button("Import").clicked();
            let enter_pressed =
                path_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if import_clicked || enter_pressed {
                match std::fs::read_to_string(app.import_buf.trim()) {
                    Ok(text) => match crate::packs::parse_pack(&text) {
                        Ok(pack) => {
                            app.store.data.packs.retain(|p| p.id != pack.id);
                            app.store.data.packs.push(pack);
                            let _ = app.store.save();
                            app.mark_dirty();
                            app.toast("Pack imported", GOOD);
                        }
                        Err(e) => app.toast(e, BAD),
                    },
                    Err(e) => app.toast(format!("Cannot read file: {e}"), BAD),
                }
            }
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new("State lives in state.json next to this folder; corrupt files are quarantined automatically.")
                .size(12.0)
                .color(FAINT),
        );
    });

    // --- About ---------------------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new(concat!("RTOM v", env!("CARGO_PKG_VERSION"))).size(14.0).strong());
        ui.label(
            RichText::new("Pure-Rust native app · no polling · single-file JSON state\nBuilt to idle in your tray at nearly zero cost.")
                .size(12.0)
                .color(FAINT),
        );
    });
}

/// Returns true when toggled.
fn setting_row(ui: &mut egui::Ui, title: &str, hint: &str, val: &mut bool) -> bool {
    let before = *val;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(ui.available_width() - 60.0);
            ui.label(RichText::new(title).size(14.5));
            ui.label(RichText::new(hint).size(11.5).color(FAINT));
        });
        toggle(ui, val);
    });
    *val != before
}

/// Writes the solved-problem table as CSV next to state.json.
pub fn export_progress_csv(
    data: &crate::store::Data,
    path: &std::path::Path,
) -> Result<usize, String> {
    fn esc(field: &str) -> String {
        if field.contains(',') || field.contains('"') || field.contains('\n') {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    }
    let mut rows = String::from("contest_id,index,name,rating,solved_at_utc\n");
    let mut count = 0usize;
    for (k, info) in &data.solved {
        let name = data
            .packs
            .iter()
            .find_map(|p| p.find(k).map(|pr| pr.name.clone()))
            .unwrap_or_default();
        let rating = data
            .packs
            .iter()
            .find_map(|p| p.find(k))
            .map(|pr| pr.rating)
            .unwrap_or(0);
        let solved_at = crate::engine::fmt_ts_date(info.ts);
        rows.push_str(&format!(
            "{},{},{},{},{}\n",
            k.contest_id,
            k.index,
            esc(&name),
            rating,
            solved_at
        ));
        count += 1;
    }
    std::fs::write(path, rows).map_err(|e| e.to_string())?;
    Ok(count)
}
