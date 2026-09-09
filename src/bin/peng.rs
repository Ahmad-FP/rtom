//! peng — GUI entrypoint. Tray-resident personal accountability app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use chrono::Utc;
use eframe::egui;
use peng_lib::engine;
use peng_lib::model::date_from_ce;
use peng_lib::platform;
use peng_lib::store::Store;
use peng_lib::ui::{theme, PengApp};

struct Wrap {
    app: PengApp,
    hide_pending: bool,
    open_sync_fired: bool,
}

impl eframe::App for Wrap {
    /// Ticks while the window is hidden (eframe runs no egui pass then, so
    /// `ui` never fires). An un-canceled close here would destroy the
    /// viewport — e.g. `taskkill` without `/F` — leaving a windowless zombie.
    /// Only the tray menu may quit the app.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // First frame rendered: tell a probing parent that this pipeline works.
        let _ = std::fs::write(heartbeat_path(), b"ok");
        self.app.today_ord = engine::day_ord_of_ts(self.app.now_ts, self.app.utc_off);

        #[cfg(feature = "tray")]
        while let Some(action) = platform::tray::try_action() {
            match action {
                platform::tray::TrayAction::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                platform::tray::TrayAction::Refresh => self.app.trigger_refresh(),
                platform::tray::TrayAction::OpenNext => {
                    if let Some(url) = self.app.dash.next_up_url.clone() {
                        platform::open_url(&url);
                    }
                }
                platform::tray::TrayAction::Quit => {
                    std::process::exit(0);
                }
            }
        }
        // Stashed tray actions from the watcher thread (menu used while the
        // viewport was hidden and `ui()` wasn't running to poll it).
        if platform::paths::tray_refresh_file().exists() {
            let _ = std::fs::remove_file(platform::paths::tray_refresh_file());
            self.app.trigger_refresh();
        }
        if platform::paths::tray_next_file().exists() {
            let _ = std::fs::remove_file(platform::paths::tray_next_file());
            if let Some(url) = self.app.dash.next_up_url.clone() {
                platform::open_url(&url);
            }
        }

        if self.hide_pending {
            self.hide_pending = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Close-to-tray: the app keeps living in the background.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Restore requests win over the hide above when both land same frame.
        // Sources: tray left-click, or a second launch (shortcut / taskbar
        // pin) touching the show-request sentinel while this process holds
        // app.lock. Checked here as well as on the watcher thread so a
        // restore works even if one path stalls.
        #[cfg(feature = "tray")]
        if platform::tray::try_show_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if platform::paths::show_request_file().exists() {
            let _ = std::fs::remove_file(platform::paths::show_request_file());
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        self.app.draw(ui);

        // Optional one-shot sync on open (explicit setting; still no polling).
        // Session-scoped: the *setting* stays on, only this launch consumes it.
        if self.app.store.data.settings.refresh_on_open
            && !self.app.syncing
            && !self.open_sync_fired
        {
            self.open_sync_fired = true;
            self.app.trigger_refresh();
        }
    }
}

/// First-run bootstrap: freeze timezone, embed CP-31 and create the standard
/// goal dated *today*, exactly once — the two-month clock starts when peng starts.
fn bootstrap(st: &mut Store) {
    if st.data.settings.utc_offset_minutes.is_none() {
        
        let off = chrono::Local::now().offset().local_minus_utc() / 60;
        st.data.settings.utc_offset_minutes = Some(off);
    }
    if !st.data.packs.iter().any(|p| p.id == "cp31") {
        st.data.packs.push(peng_lib::packs::embedded_cp31());
    }
    let now = Utc::now().timestamp();
    let today_ord =
        engine::day_ord_of_ts(now, st.data.settings.utc_offset_minutes.unwrap_or(0));
    if st.data.programs.is_empty() {
        if let Some(pack) = st.pack_by_id("cp31") {
            st.data.programs.push(engine::standard_cp31_program(
                pack,
                date_from_ce(today_ord),
                now,
            ));
        }
    }
    let _ = st.save();
}

fn launch_log(msg: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
            .open(std::env::temp_dir().join("peng-launch.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
        }
    }

fn main() {
    // Run DPI-unaware: winit's dynamic rescaling double-applies the 125% factor
    // on this machine (surface larger than window -> clipped/blank output).
    // Unaware mode makes logical == physical everywhere; DWM stretches for us.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwareness, PROCESS_DPI_UNAWARE};
        let _ = SetProcessDpiAwareness(PROCESS_DPI_UNAWARE);
    }
    // Second launch fast-path: if another instance already holds app.lock,
    // signal it to restore its window instead of spawning a useless GPU
    // probe child. Runs before the stage probe so relaunching a hidden app
    // (shortcut / taskbar pin) feels instant.
    {
        let dir = platform::paths::ensure_data_dir();
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(dir.join("app.lock"))
        {
            if f.try_lock().is_err() {
                let _ = std::fs::write(dir.join("show-request"), b"show");
                launch_log("second launch: signaled running instance");
                return;
            }
        }
    }
    // Renderer strategy: hardware GPU first, WARP (Microsoft Basic Render Drive)
    // as automatic fallback. Some Intel iGPU drivers crash or present blank
    // through both GL and DX12; WARP is rock-solid and peng's UI is tiny.
    // Renderer strategy: glow (lightest) -> wgpu/DX12 hardware -> WARP.
    // Some Intel iGPU drivers crash or blank through GL *and* hardware DX12;
    // WARP (Microsoft Basic Render Drive) always works and peng's UI is tiny.
    let stage_env = std::env::var("PENG_STAGE").unwrap_or_default();
    // Glow is white-on-present on this Intel driver; WGPU/DX12 is the only
    // pipeline verified to reach the screen, with WARP as guaranteed fallback.
    let _renderer_note = if stage_env == "glow" { "deprecated" } else { "wgpu" };
    let renderer = eframe::Renderer::Wgpu;
    if stage_env.is_empty() && std::env::var("PENG_WARP").is_err() {
        let exe = std::env::current_exe().expect("exe path");
        let hb = heartbeat_path();
        'stages: for stage in ["hw"] {
            launch_log(&format!("parent: probing stage '{stage}'"));
            let _ = std::fs::remove_file(&hb);
            let mut child = std::process::Command::new(&exe)
                .env("PENG_STAGE", stage)
                .spawn()
                .expect("spawn stage probe");
            for _ in 0..32 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                match child.try_wait() {
                    Ok(Some(code)) => {
                        launch_log(&format!("stage '{stage}' exited code={code}"));
                        continue 'stages;
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
                if hb.exists() {
                    launch_log(&format!("stage '{stage}' heartbeat ok; keeping child"));
                    return; // healthy pipeline: stay in the child, parent exits
                }
            }
            launch_log(&format!("stage '{stage}' no heartbeat; killing"));
            let _ = child.kill();
            let _ = child.wait();
        }
        // Everything failed: WARP in-process, guaranteed to work on Windows.
        launch_log("final fallback: in-process WARP");
        std::env::set_var("WGPU_ADAPTER_NAME", "Microsoft");
        std::env::set_var("PENG_STAGE", "warp");
        std::env::set_var("PENG_WARP", "1");
    }

    let start_hidden =
        std::env::args().any(|a| a == "--minimized" || a == "--hidden");

    let dir = platform::paths::ensure_data_dir();
    let mut st = Store::load(&dir);

    // Single instance: only the final UI process takes this lock. The
    // GPU stage-probe parent exits before reaching it; a second launch of
    // the app signals the running instance (see fast-path above) and exits.
    let instance_lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(dir.join("app.lock"))
        .expect("open single-instance lock file");
    if instance_lock.try_lock().is_err() {
        // Lost a race with another starting instance: still signal restore
        // so the user never gets a silent no-op launch.
        let _ = std::fs::write(dir.join("show-request"), b"show");
        launch_log("second launch blocked: already running");
        return;
    }
    // Stale restore request from a previous crash: drop it so we don't
    // pop open spuriously on a fresh boot.
    let _ = std::fs::remove_file(dir.join("show-request"));
    let _ = std::fs::remove_file(dir.join("tray-refresh-request"));
    let _ = std::fs::remove_file(dir.join("tray-next-request"));

    bootstrap(&mut st);

    #[cfg(feature = "tray")]
    platform::tray::spawn_tray();

    let hide_on_start = start_hidden || st.data.settings.start_minimized;

    let icon = {
        #[cfg(feature = "tray")]
        {
            let (rgba, w, h) = platform::tray_icon_img::icon_rgba();
            Some(egui::IconData {
                width: w,
                height: h,
                rgba,
            })
        }
        #[cfg(not(feature = "tray"))]
        None
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1080.0, 700.0])
        .with_min_inner_size([700.0, 460.0])
        .with_title("RTOM");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    // wgpu's default allocator over-reserves huge staging buffers; peng's
    // UI is tiny, so favor memory usage over performance.
    let wgpu_configuration = eframe::WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(eframe::egui_wgpu::WgpuSetupCreateNew {
            instance_descriptor: eframe::wgpu::InstanceDescriptor {
                // DX12 only: Intel's GL/Vulkan paths are unreliable here and
                // WARP (the dx12 software adapter) is our guaranteed fallback.
                backends: eframe::wgpu::Backends::DX12,
                flags: eframe::wgpu::InstanceFlags::default(),
                display: None,
                memory_budget_thresholds: Default::default(),
                backend_options: Default::default(),
            },
            display_handle: None,
            power_preference: Default::default(),
            native_adapter_selector: None,
            device_descriptor: std::sync::Arc::new(|_adapter| eframe::wgpu::DeviceDescriptor {
                label: Some("peng".into()),
                required_features: Default::default(),
                required_limits: Default::default(),
                experimental_features: Default::default(),
                memory_hints: eframe::wgpu::MemoryHints::MemoryUsage,
                trace: Default::default(),
            }),
        }),
        ..Default::default()
    };

    let opts = eframe::NativeOptions {
        renderer,
        viewport,
        wgpu_options: wgpu_configuration,
        ..Default::default()
    };

    let mut app = PengApp::new(st);
    #[cfg(debug_assertions)]
    if let Ok(h) = std::env::var("PENG_SYNC") {
        if !h.is_empty() {
            app.store.data.settings.handle = h;
            let _ = app.store.save();
            app.trigger_refresh();
        }
    }
    #[cfg(debug_assertions)]
    if std::env::var("PENG_EXPORT").is_ok() {
        let dir = platform::paths::data_dir();
        let path = dir.join("peng-export-test.csv");
        match peng_lib::ui::settings::export_progress_csv(&app.store.data, &path) {
            Ok(n) => eprintln!("[qa] exported {} rows to {}", n, path.display()),
            Err(e) => eprintln!("[qa] export failed: {e}"),
        }
    }
    // Debug-only visual-QA hooks.
    #[cfg(debug_assertions)]
    {
        if std::env::var("PENG_POPUP").is_ok() {
            app.goal_detail_open = true;
        }
        if std::env::var("PENG_DETAIL").is_ok() {
            if let Some(prog) = app.store.data.programs.first() {
                app.screen = peng_lib::ui::Screen::ProgramDetail(prog.id.clone());
            }
        }
        if std::env::var("PENG_EDITOR").is_ok() {
            // Open the first unsolved CP-31 problem in the editor.
            let target = app
                .store
                .data
                .packs
                .iter()
                .find(|p| p.id == "cp31")
                .and_then(|pack| {
                    pack.sheets
                        .iter()
                        .flat_map(|s| s.problems.iter())
                        .find(|p| !app.store.data.solved.contains_key(&p.key))
                        .map(|p| p.key.clone())
                });
            if let Some(key) = target {
                app.open_editor(&key);
            }
        }
        if let Ok(s) = std::env::var("PENG_SCREEN") {
            app.screen = match s.as_str() {
                "programs" => peng_lib::ui::Screen::Programs,
                "problems" => peng_lib::ui::Screen::Problems,
                "settings" => peng_lib::ui::Screen::Settings,
                _ => peng_lib::ui::Screen::Dashboard,
            };
        }
    }
    let result = eframe::run_native(
        "RTOM",
        opts,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            // Watcher thread: restores the window on tray left-click or a
            // second launch even when eframe stops calling `ui()` while the
            // viewport is hidden (close-to-tray). Viewport commands are
            // thread-safe, so this never needs `&mut app`.
            {
                let ctx = cc.egui_ctx.clone();
                let _ = std::thread::Builder::new()
                    .name("peng-watcher".into())
                    .spawn(move || loop {
                        let mut show = false;
                        if platform::paths::show_request_file().exists() {
                            let _ = std::fs::remove_file(platform::paths::show_request_file());
                            show = true;
                        }
                        #[cfg(feature = "tray")]
                        if platform::tray::try_show_requested() {
                            show = true;
                        }
                        // Menu events arriving while hidden: `ui()` isn't
                        // running to poll them, so consume here. Open restores
                        // directly; Refresh/OpenNext need `&mut app`, so stash
                        // a sentinel the UI thread picks up on its next frame.
                        #[cfg(feature = "tray")]
                        {
                            use tray_icon::menu::MenuEvent;
                            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                                match ev.id.0.as_str() {
                                    platform::tray::ID_OPEN => show = true,
                                    platform::tray::ID_SYNC => {
                                        let _ = std::fs::write(
                                            platform::paths::tray_refresh_file(),
                                            b"refresh",
                                        );
                                    }
                                    platform::tray::ID_NEXT => {
                                        let _ = std::fs::write(
                                            platform::paths::tray_next_file(),
                                            b"next",
                                        );
                                    }
                                    platform::tray::ID_QUIT => std::process::exit(0),
                                    _ => {}
                                }
                            }
                        }
                        if show {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                            ctx.request_repaint();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    });
            }
            Ok(Box::new(Wrap { app, hide_pending: hide_on_start, open_sync_fired: false }))
        }),
    );
    if let Err(_e) = result {
        std::process::exit(1);
    }
}

/// Heartbeat file proving the hardware pipeline rendered its first frame.
fn heartbeat_path() -> std::path::PathBuf {
    std::env::temp_dir().join("peng-hw-heartbeat")
}
