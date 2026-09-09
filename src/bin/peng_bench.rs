//! Deterministic benchmark harness for peng.
//!
//! Simulates ~2 months of usage against the real embedded CP-31 pack using
//! fixture payloads through the *real* parser/merge paths, then drives the real
//! UI code headlessly (egui run + tessellation) and reports memory/frame metrics.
//!
//! Deterministic: fixed seed, fixed dates, no network, no wall-clock dependence.

use chrono::NaiveDate;
use peng_lib::cf::{fixtures, parse_profile, parse_submissions};
use peng_lib::engine;
use peng_lib::model::ProblemKey;
use peng_lib::packs;
use peng_lib::store::Store;
use peng_lib::ui::{PengApp, Screen};
use std::path::Path;
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

// ---------------------------------------------------------------------------
// Memory metrics (OS counters)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod mem {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn peak_rss_bytes() -> u64 {
        unsafe {
            let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            GetProcessMemoryInfo(GetCurrentProcess() as HANDLE, &mut pmc, pmc.cb);
            pmc.PeakWorkingSetSize as u64
        }
    }

    pub fn current_rss_bytes() -> u64 {
        unsafe {
            let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            GetProcessMemoryInfo(GetCurrentProcess() as HANDLE, &mut pmc, pmc.cb);
            pmc.WorkingSetSize as u64
        }
    }
}

#[cfg(unix)]
mod mem {
    pub fn peak_rss_bytes() -> u64 {
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_SELF, &mut ru);
            #[cfg(target_os = "macos")]
            return ru.ru_maxrss as u64; // bytes on macOS
            #[cfg(all(unix, not(target_os = "macos")))]
            return ru.ru_maxrss as u64 * 1024; // KiB on Linux
        }
    }

    pub fn current_rss_bytes() -> u64 {
        peak_rss_bytes()
    }
}

const MB: f64 = 1024.0 * 1024.0;

/// Bench simulates the target user (IST, +5:30) for full determinism.
const BENCH_UTC_OFFSET_MINUTES: i32 = 330;

fn main() {
    let quick = std::env::args().any(|a| a == "--quick");
    let days: usize = 60;
    let frames: usize = if quick { 80 } else { 240 };
    let colds: usize = 5;

    // Isolated data dir for this run.
    let tmp = std::env::temp_dir().join(format!("peng-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create bench dir");

    // ---- Phase A: seed baseline state -------------------------------------
    let pack = packs::embedded_cp31();
    let order: Vec<i64> = {
        let mut o: Vec<i64> = pack
            .sheets
            .iter()
            .map(|s| s.rating)
            .filter(|r| *r <= engine::CP31_STANDARD_MAX_RATING)
            .collect();
        o.sort_unstable();
        o
    };
    let scope: Vec<ProblemKey> = pack.scope_problems(&order).iter().map(|p| p.key.clone()).collect();
    let scope_len = scope.len();

    let t0_date = NaiveDate::from_ymd_opt(2026, 6, 1).expect("fixed date");
    let t0_ts = t0_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp();

    {
        let mut st = Store::load(&tmp);
        st.data.programs.push(engine::standard_cp31_program(&pack, t0_date, t0_ts));
        st.save().expect("seed save");
    }

    // ---- Phase B: simulate 60 days of solves through real parse+merge ------
    let mut rng = Rng(1337);
    let mut sub_id = 10_000_000i64;
    let mut ptr = 0usize;
    let mut profile = parse_profile(&fixtures::profile_json("bench_user", 1180)).unwrap();
    let mut st = Store::load(&tmp);
    for d in 0..days {
        let n = match rng.next() % 10 {
            0..=3 => 1,
            4..=6 => 2,
            7..=8 => 3,
            _ => 0,
        } as usize;
        let mut entries: Vec<(ProblemKey, i64, i64)> = Vec::with_capacity(n);
        for _ in 0..n {
            if ptr >= scope_len {
                break;
            }
            let key = scope[ptr].clone();
            ptr += 1;
            let ts = t0_ts + d as i64 * 86_400 + (rng.next() % 80_000) as i64;
            entries.push((key, sub_id, ts));
            sub_id += 1;
        }
        if !entries.is_empty() {
            let json = fixtures::status_json(&entries);
            let subs = parse_submissions(&json).expect("fixture must parse");
            let now = t0_ts + d as i64 * 86_400 + 83_000;
            st.apply_sync("bench", profile.clone(), Vec::new(), &subs, now, BENCH_UTC_OFFSET_MINUTES);
        }
        if d % 7 == 0 && d > 0 {
            let rating = (1180 + d as i64 * 9).min(1666);
            profile = parse_profile(&fixtures::profile_json("bench_user", rating)).unwrap();
        }
        if d % 10 == 9 {
            st.save().unwrap();
        }
    }
    let final_ts = t0_ts + (days as i64 - 1) * 86_400 + 83_000;
    let final_day = engine::day_ord_of_ts(final_ts, BENCH_UTC_OFFSET_MINUTES);

    // ---- Phase B2: stress the cache toward its real-world cap ----------------
    // A year of daily syncing accumulates thousands of cached submissions.
    // Fill to the 8k cache limit with non-OK verdicts (solved set untouched).
    let stress_now = t0_ts + days as i64 * 86_400;
    {
        let target_total = 8_000usize;
        let have = st.data.submissions.len();
        if have < target_total {
            let need = target_total - have;
            let mut chunk: Vec<(ProblemKey, i64, i64)> = Vec::with_capacity(500);
            for _i in 0..need {
                let key = scope[rng.next() as usize % scope_len].clone();
                let ts = t0_ts + (rng.next() % (days as u64 * 86_400)) as i64;
                chunk.push((key, sub_id, ts));
                sub_id += 1;
                if chunk.len() == 500 {
                    let json = fixtures::status_json_verdict(&chunk, "WRONG_ANSWER");
                    let subs = parse_submissions(&json).expect("fixture must parse");
                    st.apply_sync("bench", profile.clone(), Vec::new(), &subs, stress_now, BENCH_UTC_OFFSET_MINUTES);
                    chunk.clear();
                }
            }
            if !chunk.is_empty() {
                let json = fixtures::status_json_verdict(&chunk, "WRONG_ANSWER");
                let subs = parse_submissions(&json).expect("fixture must parse");
                st.apply_sync("bench", profile.clone(), Vec::new(), &subs, stress_now, BENCH_UTC_OFFSET_MINUTES);
            }
        }
    }
    st.save().unwrap();
    drop(st);

    let state_path = tmp.join("state.json");
    let subs_path = tmp.join("submissions.json");
    let state_kb = [state_path, subs_path]
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum::<u64>() as f64
        / 1024.0;

    // ---- Phase C: cold-start measurement ------------------------------------
    let ctx = egui::Context::default();
    let make_app = |tmp: &Path| {
        let store = Store::load(tmp);
        let mut app = PengApp::new(store);
        app.set_clock(final_ts, final_day);
        app
    };

    // Warm the shared egui context (fonts/shaders-independent CPU caches).
    let draw_once = |app: &mut PengApp, ctx: &egui::Context| {
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1150.0, 760.0));
        let input = egui::RawInput {
            time: Some(0.0),
            screen_rect: Some(screen_rect),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |root_ui| {
            app.draw(root_ui);
        });
        std::hint::black_box(ctx.tessellate(out.shapes, out.pixels_per_point).len());
    };
    {
        let mut warm = make_app(&tmp);
        draw_once(&mut warm, &ctx);
    }
    let mut startup_ms = Vec::with_capacity(colds);
    for _ in 0..colds {
        let t = Instant::now();
        let mut app = make_app(&tmp);
        draw_once(&mut app, &ctx);
        startup_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    startup_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // ---- Phase D: frame cost over every screen -------------------------------
    let ratings_len = pack.sheets.len().max(1);
    let mut app = make_app(&tmp);
    let mut samples_us: Vec<u64> = Vec::with_capacity(frames);
    for i in 0..frames {
        app.screen = {
            let cycle = [
                Screen::Dashboard,
                Screen::Programs,
                Screen::Problems,
                Screen::Settings,
            ];
            // Include the detail page in the rotation when state allows it.
            if !app.store.data.programs.is_empty() && i % 8 == 3 {
                Screen::ProgramDetail(app.store.data.programs[0].id.clone())
            } else {
                cycle[i % 4].clone()
            }
        };
        if !pack.sheets.is_empty() {
            app.prob_sheet = i % ratings_len;
        }
        // Halfway through, apply a real sync so the measured p95 includes a
        // dashboard rebuild over the full 8k cache — as happens after refresh.
        if i == frames / 2 {
            let keys: Vec<ProblemKey> = scope[..3].to_vec();
            let batch: Vec<(ProblemKey, i64, i64)> = keys
                .iter()
                .enumerate()
                .map(|(k, key)| (key.clone(), sub_id + k as i64, final_ts + 60))
                .collect();
            sub_id += 3;
            let json = fixtures::status_json(&batch);
            let subs = parse_submissions(&json).expect("fixture must parse");
            app.store.apply_sync(
                "bench",
                profile.clone(),
                Vec::new(),
                &subs,
                final_ts + 120,
                BENCH_UTC_OFFSET_MINUTES,
            );
            let _ = app.store.save();
            app.mark_dirty();
        }
        let input = egui::RawInput {
            time: Some(i as f64 * 0.016),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1150.0, 760.0),
            )),
            ..Default::default()
        };
        let t = Instant::now();
        let out = ctx.run_ui(input, |root_ui| {
            app.draw(root_ui);
        });
        let meshes = ctx.tessellate(out.shapes, out.pixels_per_point);
        samples_us.push(t.elapsed().as_micros() as u64);
        std::hint::black_box(meshes.len());
    }
    samples_us.sort_unstable();
    let avg_ms = samples_us.iter().sum::<u64>() as f64 / samples_us.len() as f64 / 1000.0;
    let p95_idx = samples_us.len() * 95 / 100;
    let p95_ms = samples_us[p95_idx.min(samples_us.len() - 1)] as f64 / 1000.0;

    // ---- Phase E: report ------------------------------------------------------
    let peak_mb = mem::peak_rss_bytes() as f64 / MB;
    let steady_mb = mem::current_rss_bytes() as f64 / MB;
    let solved = Store::load(&tmp).data.solved.len();

    println!("METRIC peak_rss_mb={:.1}", peak_mb);
    println!("METRIC startup_ms={:.0}", startup_ms[startup_ms.len() / 2]);
    println!("METRIC ui_frame_avg_ms={:.3}", avg_ms);
    println!("METRIC ui_frame_p95_ms={:.3}", p95_ms);
    println!("METRIC state_size_kb={:.1}", state_kb);
    println!(
        "ASI simulated_days={} frames={} scope_problems={} solved_in_sim={} steady_rss_mb={:.1} cache_submissions={}",
        days, frames, scope_len, solved, steady_mb, app.store.data.submissions.len()
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
