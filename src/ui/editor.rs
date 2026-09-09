//! Code editor — write solutions, run them locally against sample tests or
//! custom input, and submit to Codeforces on your own account.
//!
//! Login is session-only: the password is taken from the UI buffer for one
//! POST and dropped; cookies live in `PengApp::cf_session` (memory) and die
//! with the process. Nothing credential-shaped is ever persisted.

use super::{card, chip, heading, theme::*, PengApp};
use crate::cf::SubmitOutcome;
use crate::model::{editor_key, EditorFile, ProblemKey};
use crate::runner::{self, CaseVerdict};
use egui::{Color32, RichText};

#[derive(Clone, Debug, Default)]
pub enum SubmitState {
    #[default]
    Idle,
    Sending,
    Sent { at_ts: i64 },
    Verdict { text: String, ok: bool },
    Failed(String),
}

fn save_editor(app: &mut PengApp) {
    if let Err(e) = app.store.save() {
        app.toast(format!("Save failed: {e}"), BAD);
    }
}

fn ensure_toolchains(app: &mut PengApp) {
    if app.toolchains.is_none() {
        app.toolchains = Some(runner::detect_toolchains());
    }
}

fn exe_for(app: &PengApp, lang: &str) -> Option<String> {
    app.toolchains
        .as_ref()?
        .iter()
        .find(|t| t.lang == lang)
        .map(|t| t.exe.clone())
}

fn load_samples(app: &mut PengApp, key: &ProblemKey) -> Result<usize, String> {
    let had_session = app.cf_session.is_some();
    let mut web = match app.cf_session.take() {
        Some(s) => s,
        None => crate::cf::WebSession::anonymous(),
    };
    let html = web.fetch_problem_html(key.contest_id, &key.index);
    if had_session {
        app.cf_session = Some(web);
    }
    let samples = runner::parse_samples(&html?);
    if samples.is_empty() {
        return Err("no sample tests found on the problem page".into());
    }
    let n = samples.len();
    app.store.data.editor_samples.insert(editor_key(key), samples);
    Ok(n)
}

pub fn friendly_verdict(v: &str) -> String {
    match v {
        "OK" => "Accepted".into(),
        "WRONG_ANSWER" => "Wrong answer".into(),
        "TIME_LIMIT_EXCEEDED" => "Time limit exceeded".into(),
        "MEMORY_LIMIT_EXCEEDED" => "Memory limit exceeded".into(),
        "RUNTIME_ERROR" => "Runtime error".into(),
        "COMPILATION_ERROR" => "Compilation error".into(),
        "TESTING" => "Still judging…".into(),
        "CHALLENGED" => "Challenged".into(),
        "SKIPPED" => "Skipped".into(),
        "REJECTED" => "Rejected".into(),
        other => other.replace('_', " "),
    }
}

pub fn show(app: &mut PengApp, ui: &mut egui::Ui, key_str: &str) {
    let Some(key) = crate::model::parse_editor_key(key_str) else {
        heading(ui, "Editor");
        card(ui, |ui| {
            ui.label(RichText::new("Unknown problem.").color(DIM));
        });
        return;
    };
    let kstr = editor_key(&key);

    // Snapshot problem metadata (owned; the store borrow ends here).
    let (pname, prating) = app
        .store
        .data
        .packs
        .iter()
        .find_map(|p| p.find(&key))
        .map(|p| (p.name.clone(), p.rating))
        .unwrap_or((format!("{}/{}", key.contest_id, key.index), 0));
    let solved = app.store.data.solved.contains_key(&key);
    if app.login_handle_buf.is_empty() && !app.store.data.settings.handle.is_empty() {
        app.login_handle_buf = app.store.data.settings.handle.clone();
    }
    ensure_toolchains(app);

    if ui.button(RichText::new("< Back").size(13.0).color(ACCENT2)).clicked() {
        save_editor(app);
        let back = app.editor_return.clone();
        app.screen = back;
        return;
    }
    heading(ui, "Editor");
    ui.horizontal(|ui| {
        ui.label(RichText::new(&pname).size(17.0).strong());
        if prating > 0 {
            let t = crate::engine::cf_rank(Some(prating)).color;
            chip(ui, &prating.to_string(), Color32::from_rgb(t[0], t[1], t[2]));
        }
        if solved {
            chip(ui, "solved", GOOD);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Open on Codeforces >").clicked() {
                super::Opener::open(&key.url());
            }
        });
    });
    ui.add_space(4.0);

    // --- Session ------------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("CODEFORCES SESSION").size(11.0).color(FAINT));
        let session_name = app.cf_session.as_ref().map(|s| s.handle_name().to_string());
        if let Some(name) = session_name {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Logged in as {name} (this session only)"))
                        .size(13.5)
                        .color(GOOD),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log out").clicked() {
                        app.cf_session = None;
                        app.submit_state = SubmitState::Idle;
                        app.toast("Logged out — session destroyed", DIM);
                    }
                });
            });
            ui.label(
                RichText::new("Closing the app ends the session. Nothing was stored.")
                    .size(12.0)
                    .color(FAINT),
            );
        } else {
            ui.label(
                RichText::new("Log in once to submit from the app. Password is used for one request and never stored.")
                    .size(12.5)
                    .color(DIM),
            );
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Handle").color(DIM));
                ui.add(
                    egui::TextEdit::singleline(&mut app.login_handle_buf).desired_width(170.0),
                );
                ui.label(RichText::new("Password").color(DIM));
                ui.add(
                    egui::TextEdit::singleline(&mut app.login_pass_buf)
                        .password(true)
                        .desired_width(170.0),
                );
                let btn = egui::Button::new(RichText::new(if app.authing {
                    "Logging in…"
                } else {
                    "Log in"
                }))
                .fill(ACCENT);
                if ui.add_enabled(!app.authing, btn).clicked() {
                    app.trigger_login();
                }
            });
        }
    });
    ui.add_space(8.0);

    // --- Language + source ---------------------------------------------------
    let (lang_now, have_cpp, have_py) = {
        let f = app
            .store
            .data
            .editor_files
            .entry(kstr.clone())
            .or_insert_with(|| EditorFile {
                lang: runner::LANG_CPP.into(),
                source: runner::template_for(runner::LANG_CPP).into(),
            });
        let tcs = app.toolchains.clone().unwrap_or_default();
        (
            f.lang.clone(),
            tcs.iter().any(|t| t.lang == runner::LANG_CPP),
            tcs.iter().any(|t| t.lang == runner::LANG_PY),
        )
    };
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Language").color(DIM));
            let mut lang = lang_now.clone();
            egui::ComboBox::from_id_salt("ed-lang")
                .selected_text(if lang == runner::LANG_PY { "Python" } else { "C++" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut lang, runner::LANG_CPP.into(), "C++");
                    ui.selectable_value(&mut lang, runner::LANG_PY.into(), "Python");
                });
            if lang != lang_now {
                if let Some(f) = app.store.data.editor_files.get_mut(&kstr) {
                    f.lang = lang.clone();
                    if f.source.trim().is_empty() {
                        f.source = runner::template_for(&lang).into();
                    }
                }
                app.ed_report = None;
                save_editor(app);
            }
            if lang == runner::LANG_CPP && !have_cpp {
                ui.label(RichText::new("g++ not found").size(12.0).color(WARN));
            }
            if lang == runner::LANG_PY && !have_py {
                ui.label(RichText::new("python not found").size(12.0).color(WARN));
            }
        });
        ui.add_space(2.0);
        let mut save_after = false;
        if let Some(f) = app.store.data.editor_files.get_mut(&kstr) {
            let resp = ui.add(
                egui::TextEdit::multiline(&mut f.source)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(22)
                    .desired_width(f32::INFINITY),
            );
            if resp.changed() {
                save_after = true;
            }
        }
        if save_after {
            app.ed_report = None;
            save_editor(app);
        }
    });
    ui.add_space(8.0);

    // --- Samples --------------------------------------------------------------
    let sample_count = app
        .store
        .data
        .editor_samples
        .get(&kstr)
        .map(|s| s.len())
        .unwrap_or(0);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SAMPLE TESTS").size(11.0).color(FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Load samples").clicked() {
                    match load_samples(app, &key) {
                        Ok(n) => {
                            app.toast(format!("Loaded {n} sample tests"), GOOD);
                            save_editor(app);
                        }
                        Err(e) => app.toast(format!("Samples failed: {e}"), BAD),
                    }
                }
            });
        });
        if sample_count == 0 {
            ui.label(
                RichText::new("No samples yet — load them from the problem statement (one fetch, cached offline).")
                    .size(12.5)
                    .color(DIM),
            );
        } else if ui
            .button(RichText::new(format!("Run {sample_count} samples")).size(14.0))
            .clicked()
        {
            run_samples(app, &kstr, &key);
        }
    });
    ui.add_space(4.0);

    // Last sample report for this problem.
    if let Some((rk, rep)) = app.ed_report.clone() {
        if rk == kstr {
            paint_report(ui, &rep, app.store.data.editor_samples.get(&kstr).map(|v| v.as_slice()).unwrap_or(&[]));
        }
    }

    // --- Custom input ----------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("CUSTOM INPUT").size(11.0).color(FAINT));
        ui.add(
            egui::TextEdit::multiline(&mut app.ed_custom_input)
                .font(egui::TextStyle::Monospace)
                .hint_text("stdin for your program…")
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(2.0);
        if ui.button(RichText::new("Run with custom input").size(13.5)).clicked() {
            run_custom(app, &kstr);
        }
        if let Some(rep) = app.ed_custom.clone() {
            paint_custom(ui, &rep);
        }
    });
    ui.add_space(8.0);

    // --- Submit -----------------------------------------------------------------
    card(ui, |ui| {
        ui.label(RichText::new("SUBMIT TO CODEFORCES").size(11.0).color(FAINT));
        let logged_in = app.cf_session.is_some();
        let sending = matches!(app.submit_state, SubmitState::Sending);
        if !logged_in {
            ui.label(
                RichText::new("Log in above to submit as yourself. Your verdict lands in your real CF history; Refresh then records the AC.")
                    .size(12.5)
                    .color(DIM),
            );
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let btn = egui::Button::new(RichText::new(if sending {
                "Submitting…"
            } else {
                "Submit solution"
            }))
            .fill(ACCENT);
            if ui.add_enabled(logged_in && !sending, btn).clicked() {
                send_solution(app, &kstr, &key);
            }
            match &app.submit_state {
                SubmitState::Idle => {}
                SubmitState::Sending => {
                    ui.label(RichText::new("sending…").color(DIM));
                }
                SubmitState::Sent { .. } => {
                    ui.label(RichText::new("sent — judging").color(WARN));
                    if ui.button("Check verdict").clicked() {
                        check_verdict(app, &key);
                    }
                }
                SubmitState::Verdict { text, ok } => {
                    ui.label(
                        RichText::new(text.clone())
                            .strong()
                            .color(if *ok { GOOD } else { WARN }),
                    );
                    if !ok && ui.button("Check again").clicked() {
                        check_verdict(app, &key);
                    }
                }
                SubmitState::Failed(e) => {
                    ui.label(RichText::new(e.clone()).color(BAD));
                }
            }
        });
    });
}

fn active_file(app: &PengApp, kstr: &str) -> Option<(String, String)> {
    app.store
        .data
        .editor_files
        .get(kstr)
        .map(|f| (f.lang.clone(), f.source.clone()))
}

fn run_samples(app: &mut PengApp, kstr: &str, key: &ProblemKey) {
    let Some((lang, source)) = active_file(app, kstr) else { return };
    let samples = app
        .store
        .data
        .editor_samples
        .get(kstr)
        .cloned()
        .unwrap_or_default();
    if samples.is_empty() {
        app.toast("Load samples first", WARN);
        return;
    }
    let Some(exe) = exe_for(app, &lang) else {
        app.toast(
            if lang == runner::LANG_CPP {
                "g++ not found — install MinGW to test C++ locally"
            } else {
                "python not found — install Python to test locally"
            },
            BAD,
        );
        return;
    };
    save_editor(app);
    let inputs: Vec<String> = samples.iter().map(|s| s.input.clone()).collect();
    let mut rep = runner::run_all(&lang, &exe, &source, &inputs);
    for (i, c) in rep.cases.iter_mut().enumerate() {
        if c.verdict == CaseVerdict::Pass && !runner::judge(&samples[i].expected, &c.actual) {
            c.verdict = CaseVerdict::WrongAnswer;
        }
    }
    let passed = rep.passed();
    app.ed_report = Some((kstr.to_string(), rep));
    app.toast(
        format!("Samples: {passed}/{} passed", samples.len()),
        if passed == samples.len() { GOOD } else { WARN },
    );
    let _ = key;
}

fn run_custom(app: &mut PengApp, kstr: &str) {
    let Some((lang, source)) = active_file(app, kstr) else { return };
    let Some(exe) = exe_for(app, &lang) else {
        app.toast("No toolchain for this language", BAD);
        return;
    };
    save_editor(app);
    let input = app.ed_custom_input.clone();
    let rep = runner::run_all(&lang, &exe, &source, &[input]);
    app.ed_custom = Some(rep);
}

fn paint_report(ui: &mut egui::Ui, rep: &runner::RunReport, samples: &[crate::model::Sample]) {
    ui.add_space(2.0);
    if let Some(e) = &rep.compile_error {
        ui.label(RichText::new("Compilation failed").strong().color(BAD));
        ui.label(RichText::new(e.clone()).monospace().size(12.0).color(TEXT));
        return;
    }
    for (i, c) in rep.cases.iter().enumerate() {
        let (label, color) = match c.verdict {
            CaseVerdict::Pass => ("PASS", GOOD),
            CaseVerdict::WrongAnswer => ("WRONG ANSWER", BAD),
            CaseVerdict::RuntimeError => ("RUNTIME ERROR", BAD),
            CaseVerdict::TimeLimit => ("TIME LIMIT", WARN),
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, color);
            ui.label(RichText::new(format!("Test {}: {label} · {} ms", i + 1, c.ms)).size(13.0).color(color));
        });
        if c.verdict != CaseVerdict::Pass {
            if let Some(s) = samples.get(i) {
                ui.label(RichText::new(format!("expected:\n{}", snippet(&s.expected))).monospace().size(12.0).color(DIM));
                ui.label(RichText::new(format!("got:\n{}", snippet(&c.actual))).monospace().size(12.0).color(TEXT));
            }
            if !c.stderr.trim().is_empty() {
                ui.label(RichText::new(format!("stderr:\n{}", snippet(&c.stderr))).monospace().size(12.0).color(WARN));
            }
        }
    }
}

fn paint_custom(ui: &mut egui::Ui, rep: &runner::RunReport) {
    ui.add_space(2.0);
    if let Some(e) = &rep.compile_error {
        ui.label(RichText::new("Compilation failed").strong().color(BAD));
        ui.label(RichText::new(e.clone()).monospace().size(12.0));
        return;
    }
    if let Some(c) = rep.cases.first() {
        match c.verdict {
            CaseVerdict::TimeLimit => {
                ui.label(RichText::new(format!("Time limit ({} ms)", c.ms)).color(WARN));
            }
            CaseVerdict::RuntimeError if c.actual.is_empty() && !c.stderr.is_empty() => {
                ui.label(RichText::new("Runtime error").strong().color(BAD));
                ui.label(RichText::new(snippet(&c.stderr)).monospace().size(12.0).color(WARN));
            }
            _ => {
                ui.label(RichText::new(format!("Output ({} ms):", c.ms)).size(12.0).color(DIM));
                ui.label(RichText::new(snippet(&c.actual)).monospace().size(12.5));
                if !c.stderr.trim().is_empty() {
                    ui.label(RichText::new(format!("stderr:\n{}", snippet(&c.stderr))).monospace().size(12.0).color(WARN));
                }
            }
        }
    }
}

fn snippet(s: &str) -> String {
    const MAX: usize = 1_500;
    if s.len() <= MAX {
        return if s.is_empty() { "(empty)".into() } else { s.to_string() };
    }
    format!("{}… ({} more chars)", &s[..MAX], s.len() - MAX)
}

fn send_solution(app: &mut PengApp, kstr: &str, key: &ProblemKey) {
    let Some((lang, source)) = active_file(app, kstr) else { return };
    let Some(mut sess) = app.cf_session.take() else {
        app.toast("Log in first", WARN);
        return;
    };
    // Resolve the CF language id against the live dropdown inside the worker.
    let tx = app.submit_tx.clone();
    let key = key.clone();
    app.submit_state = SubmitState::Sending;
    std::thread::spawn(move || {
        let langs = match sess.submit_langs(key.contest_id) {
            Ok(l) => l,
            Err(e) => {
                let dead = e.contains("expired");
                let _ = tx.send(SubmitOutcome::SubmitFailed {
                    error: e,
                    session: if dead { None } else { Some(sess) },
                });
                return;
            }
        };
        let (pid, plabel) = crate::cf::pick_program_type(&lang, &langs);
        let _ = &plabel;
        match sess.submit(key.contest_id, &key.index, &pid, &source) {
            Ok(()) => {
                let at_ts = chrono::Utc::now().timestamp();
                let _ = tx.send(SubmitOutcome::Sent { at_ts, session: sess });
            }
            Err(e) => {
                let dead = e.contains("expired");
                let _ = tx.send(SubmitOutcome::SubmitFailed {
                    error: e,
                    session: if dead { None } else { Some(sess) },
                });
            }
        }
    });
}

fn check_verdict(app: &mut PengApp, key: &ProblemKey) {
    let at_ts = match &app.submit_state {
        SubmitState::Sent { at_ts } => *at_ts,
        _ => return,
    };
    let handle = app.store.data.settings.handle.trim().to_string();
    if handle.is_empty() {
        app.toast("Set your handle in Settings so the verdict can be matched", WARN);
        return;
    }
    let tx = app.submit_tx.clone();
    let key = key.clone();
    std::thread::spawn(move || {
        let client = crate::cf::CfClient::new();
        let out = (|| -> Result<(String, bool), String> {
            let subs = client.fetch_submissions(&handle, 10)?;
            let hit = subs
                .iter()
                .find(|s| s.key == key && s.ts >= at_ts - 30)
                .ok_or("no submission found yet — wait a few seconds")?;
            Ok((friendly_verdict(&hit.verdict), hit.verdict == "OK"))
        })();
        let _ = tx.send(match out {
            Ok((text, ok)) => SubmitOutcome::Verdict { text, ok },
            Err(e) => SubmitOutcome::VerdictFailed(e),
        });
    });
}
