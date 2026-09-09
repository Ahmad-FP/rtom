# RTOM

Ultra-light, tray-resident Codeforces accountability companion: training programs with
percentage goals, streaks, ranks, on-demand Codeforces sync (no polling), a per-problem
code editor with local sample testing, and session-only Codeforces submit.

## Credits

- Web login/submit protocol adapted from [cf-tool](https://github.com/xalanq/cf-tool)
  by [xalanq](https://github.com/xalanq) (MIT) — Codeforces offers no submit API, so
  RTOM drives the same HTML form a browser submits, mirroring cf-tool's field set
  (`csrf_token`, `ftaa`/`bfaa`, `_tta`, `submitSolutionFormSubmitted`).
- Standard training pack: the CP-31 sheet by [TLE Eliminators](https://www.tle-eliminators.com/cp-sheet).
- UI: [egui](https://github.com/emilk/egui).

## Privacy note

Codeforces login is session-only: the password is used for a single login POST and
never written to disk. Cookies live in app memory and die with the process.
