//! OS integration: data dir, URL opening, launch-on-login, tray icon bitmap.

use std::path::PathBuf;

pub mod paths {
    use super::PathBuf;

    pub fn data_dir() -> PathBuf {
        if let Ok(d) = std::env::var("PENG_DATA") {
            if !d.is_empty() {
                return PathBuf::from(d);
            }
        }
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("peng")
    }

    pub fn ensure_data_dir() -> PathBuf {
        let d = data_dir();
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// Sentinel file a second launch touches to ask the running instance
    /// to restore its window (close-to-tray keeps one process alive holding
    /// `app.lock`, so a shortcut/taskbar relaunch would otherwise exit
    /// silently and look dead).
    pub fn show_request_file() -> PathBuf {
        data_dir().join("show-request")
    }

    /// Stashed tray-menu actions. The watcher thread (which works even while
    /// the viewport is hidden) consumes `MenuEvent`s and drops one of these
    /// so the UI thread can run the action needing `&mut app` on its next
    /// frame. Each event is consumed exactly once — either here or by the
    /// UI's own `try_action` poll — so nothing double-fires.
    pub fn tray_refresh_file() -> PathBuf {
        data_dir().join("tray-refresh-request")
    }

    pub fn tray_next_file() -> PathBuf {
        data_dir().join("tray-next-request")
    }
}

/// Opens a URL (or folder path) in the system browser/file manager.
pub fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

pub mod autostart {
    /// Whether peng is registered to launch at login.
    pub fn enabled() -> bool {
        imp::enabled()
    }

    pub fn set(on: bool) {
        if on {
            imp::enable()
        } else {
            imp::disable()
        }
    }

    fn exe_path() -> String {
        format!(
            "\"{}\"",
            std::env::current_exe()
                .unwrap_or_else(|_| std::path::PathBuf::from("peng"))
                .display()
        )
    }

    #[cfg(windows)]
    mod imp {
        use super::exe_path;
        use winreg::enums::*;
        use winreg::RegKey;

        const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const NAME: &str = "Peng";

        pub fn enabled() -> bool {
            RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey(RUN_KEY)
                .ok()
                .and_then(|k| k.get_value::<String, _>(NAME).ok())
                .is_some()
        }

        pub fn enable() {
            if let Ok((k, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY) {
                let _ = k.set_value(NAME, &exe_path());
            }
        }

        pub fn disable() {
            if let Ok(k) =
                RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            {
                let _ = k.delete_value(NAME);
            }
        }
    }

    #[cfg(target_os = "macos")]
    mod imp {
        use super::exe_path;
        use std::fs;
        use std::path::PathBuf;

        fn plist() -> PathBuf {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/LaunchAgents/app.peng.plist")
        }

        pub fn enabled() -> bool {
            plist().exists()
        }

        pub fn enable() {
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
                 \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\"><dict>\n\
                 <key>Label</key><string>app.peng</string>\n\
                 <key>ProgramArguments</key><array><string>{}</string></array>\n\
                 <key>RunAtLoad</key><true/>\n\
                 </dict></plist>\n",
                exe_path().trim_matches('"')
            );
            let _ = fs::write(plist(), body);
        }

        pub fn disable() {
            let _ = fs::remove_file(plist());
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    mod imp {
        use std::fs;

        fn desktop() -> std::path::PathBuf {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".config/autostart/peng.desktop")
        }

        pub fn enabled() -> bool {
            desktop().exists()
        }

        pub fn enable() {
            let _ = fs::create_dir_all(desktop().parent().unwrap());
            let body = format!(
                "[Desktop Entry]\nType=Application\nName=Peng\nExec={}\n\
                 X-GNOME-Autostart-enabled=true\n",
                super::super::autostart::exe_path().trim_matches('"')
            );
            let _ = fs::write(desktop(), body);
        }

        pub fn disable() {
            let _ = fs::remove_file(desktop());
        }
    }
}

/// Procedurally drawn app icon — a rounded teal→blue tile with a white bolt.
/// Avoids any image-decoding dependency.
#[cfg(feature = "tray")]
pub mod tray_icon_img {
    const S: u32 = 48;

    pub fn icon_rgba() -> (Vec<u8>, u32, u32) {
        let mut px = vec![0u8; (S * S * 4) as usize];
        // Bolt outline (normalized coords, y down).
        let bolt: [(f32, f32); 7] = [
            (-0.04, -0.60),
            (0.30, -0.60),
            (0.08, -0.10),
            (0.32, -0.10),
            (-0.16, 0.64),
            (0.02, 0.12),
            (-0.24, 0.12),
        ];
        for y in 0..S {
            for x in 0..S {
                let fx = (x as f32 + 0.5) / S as f32 * 2.0 - 1.0;
                let fy = (y as f32 + 0.5) / S as f32 * 2.0 - 1.0;
                // Rounded-rect SDF, radius 0.35.
                let r = 0.35f32;
                let qx = fx.abs() - (1.0 - r);
                let qy = fy.abs() - (1.0 - r);
                let outside = qx.max(0.0).hypot(qy.max(0.0));
                let inside = qx.max(qy).min(0.0);
                let d = outside + inside - r; // <=0 inside
                if d > 0.004 {
                    continue;
                }
                let t = ((fx + fy + 2.0) / 4.0).clamp(0.0, 1.0);
                let mut c = [
                    (94.0 + (59.0 - 94.0) * t) as u8,
                    (234.0 + (130.0 - 234.0) * t) as u8,
                    (212.0 + (246.0 - 212.0) * t) as u8,
                ];
                if point_in_bolt(fx, fy, &bolt) {
                    c = [250, 250, 252];
                }
                let aa = ((1.0 - (d / 0.008).abs()).clamp(0.0, 1.0) * 255.0) as u8;
                let i = ((y * S + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = aa;
            }
        }
        (px, S, S)
    }

    fn point_in_bolt(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
        let n = poly.len();
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}


/// System-tray integration. Menu events flow through muda's global channel,
/// polled by the UI thread. Linux note: build with `--no-default-features`
/// to drop the tray entirely (no libappindicator/gtk needed).
#[cfg(feature = "tray")]
pub mod tray {
    /// Actions requested from the tray menu.
    #[derive(Clone, Copy, Debug)]
    pub enum TrayAction {
        Open,
        Refresh,
        OpenNext,
        Quit,
    }

    use std::sync::Mutex;
    use std::sync::{OnceLock};

    /// Sender to the tray thread: tooltip updates are executed there because
    /// `TrayIcon` is !Send on Windows (Rc/RefCell internals).
    static TOOLTIP_TX: OnceLock<Mutex<std::sync::mpsc::Sender<String>>> = OnceLock::new();
    static LAST_TOOLTIP: Mutex<Option<String>> = Mutex::new(None);

    /// Updates the tray tooltip (no-op when the tray failed to build).
    /// Deduplicated so per-frame calls are cheap.
    pub fn set_tooltip(text: &str) {
        let changed = {
            let mut guard = LAST_TOOLTIP.lock().unwrap_or_else(|p| p.into_inner());
            if guard.as_deref() != Some(text) {
                *guard = Some(text.to_string());
                true
            } else {
                false
            }
        };
        if !changed {
            return;
        }
        if let Some(tx) = TOOLTIP_TX.get().and_then(|m| m.lock().ok()) {
            let _ = tx.send(text.to_string());
        }
    }

    pub const ID_OPEN: &str = "peng-open";
    pub const ID_SYNC: &str = "peng-sync";
    pub const ID_NEXT: &str = "peng-next";
    pub const ID_QUIT: &str = "peng-quit";

    fn tray_log(msg: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
            .open(std::env::temp_dir().join("peng-tray.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
        }
    }

    /// Builds the tray on a dedicated thread and parks it there.
    ///
    /// The tray's hidden HWND is created on this thread, so Windows delivers
    /// its click/menu messages to *this* thread's queue — the thread must
    /// pump messages or the icon renders dead (visible, clicks do nothing).
    pub fn spawn_tray() {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let _ = TOOLTIP_TX.set(Mutex::new(tx));
        let _ = std::thread::Builder::new().name("peng-tray".into()).spawn(move || {
            use tray_icon::menu::{Menu, MenuItem};
            use tray_icon::{Icon, TrayIconBuilder};

            let menu = Menu::with_id("peng-menu");
            let items = [
                MenuItem::with_id(ID_OPEN, "Open peng", true, None),
                MenuItem::with_id(ID_SYNC, "Refresh Codeforces", true, None),
                MenuItem::with_id(ID_NEXT, "Open next problem", true, None),
                MenuItem::with_id(ID_QUIT, "Quit", true, None),
            ];
            for item in &items {
                let _ = menu.append(item);
            }

            let (rgba, w, h) = super::tray_icon_img::icon_rgba();
            let Ok(icon) = Icon::from_rgba(rgba, w, h) else {
                tray_log("tray: Icon::from_rgba failed");
                return;
            };
            let tray = match TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("RTOM — personal accountability")
                .with_icon(icon)
                .build()
            {
                Ok(t) => t,
                Err(e) => {
                    tray_log(&format!("tray: build failed: {e:?}"));
                    return;
                }
            };
            tray_log("tray: built ok");
            // TrayIcon is !Send: the handle must stay on this thread, so
            // tooltip updates are served here via the channel. The
            // recv_timeout doubles as the message-pump cadence.
            loop {
                #[cfg(windows)]
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
                    };
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                match rx.recv_timeout(std::time::Duration::from_millis(16)) {
                    Ok(text) => {
                        let _ = tray.set_tooltip(Some(text.as_str()));
                        while let Ok(t) = rx.try_recv() {
                            let _ = tray.set_tooltip(Some(t.as_str()));
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    /// Polls pending tray actions (non-blocking).
    pub fn try_action() -> Option<TrayAction> {
        use tray_icon::menu::MenuEvent;
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let action = match ev.id.0.as_str() {
                ID_OPEN => Some(TrayAction::Open),
                ID_SYNC => Some(TrayAction::Refresh),
                ID_NEXT => Some(TrayAction::OpenNext),
                ID_QUIT => Some(TrayAction::Quit),
                _ => continue,
            };
            tray_log(&format!("tray: menu event {:?}", ev.id.0));
            return action;
        }
        None
    }

    /// Left-click on the tray icon should restore the window. The context
    /// menu is OS-handled, but clicks only arrive here — previously they
    /// were ignored, so left-clicking a hidden app looked dead.
    pub fn try_show_requested() -> bool {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        let mut show = false;
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button, button_state, .. } = ev {
                tray_log(&format!("tray: click {button:?} {button_state:?}"));
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    show = true;
                }
            }
        }
        show
    }
}
