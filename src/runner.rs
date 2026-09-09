//! Local solution runner: detect C++/Python toolchains, compile and execute
//! solutions against sample tests or custom input, and judge the output.
//!
//! Everything here is blocking and runs on a worker thread or directly off a
//! button press. No network, no Codeforces credentials — this module never
//! leaves the machine.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const LANG_CPP: &str = "cpp";
pub const LANG_PY: &str = "py";

/// Per-run wall-clock budget (Codeforces limits are 1–4 s; local runs get slack).
pub const RUN_TIMEOUT_MS: u64 = 5_000;

/// A locally available language toolchain.
#[derive(Clone, Debug)]
pub struct Toolchain {
    pub lang: &'static str,
    pub display: String,
    pub exe: String,
}

fn cmd_ok(exe: &str, arg: &str) -> bool {
    Command::new(exe)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe the machine for usable compilers/interpreters (fast: `--version` only).
pub fn detect_toolchains() -> Vec<Toolchain> {
    let mut out = Vec::new();
    if cmd_ok("g++", "--version") {
        out.push(Toolchain {
            lang: LANG_CPP,
            display: "C++ (g++)".into(),
            exe: "g++".into(),
        });
    }
    // Prefer `python`, fall back to `python3` (Windows Store shims report via --version fine).
    let py = ["python", "python3"]
        .into_iter()
        .find(|exe| cmd_ok(exe, "--version"));
    if let Some(exe) = py {
        out.push(Toolchain {
            lang: LANG_PY,
            display: format!("Python ({exe})"),
            exe: exe.into(),
        });
    }
    out
}

pub fn template_for(lang: &str) -> &'static str {
    match lang {
        LANG_PY => "import sys\n\ndef solve() -> None:\n    data = sys.stdin.read().strip().split()\n    if not data:\n        return\n    print(data[0])\n\nif __name__ == \"__main__\":\n    solve()\n",
        _ => "#include <bits/stdc++.h>\nusing namespace std;\n\nint main() {\n    ios::sync_with_stdio(false);\n    cin.tie(nullptr);\n\n    return 0;\n}\n",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseVerdict {
    Pass,
    WrongAnswer,
    RuntimeError,
    TimeLimit,
}

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub verdict: CaseVerdict,
    pub actual: String,
    pub stderr: String,
    pub ms: u128,
}

#[derive(Clone, Debug, Default)]
pub struct RunReport {
    /// Set when compilation failed (C++); `cases` is then empty.
    pub compile_error: Option<String>,
    pub cases: Vec<CaseResult>,
}

impl RunReport {
    pub fn passed(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.verdict == CaseVerdict::Pass)
            .count()
    }
}

/// Normalize program output the way Codeforces checkers loosely do: trailing
/// whitespace per line is insignificant, as are leading/trailing blank lines.
pub fn normalize(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn judge(expected: &str, actual: &str) -> bool {
    normalize(expected) == normalize(actual)
}

fn run_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rtom-run-{}", std::process::id()))
}

/// Compile (C++) and run `source` against each input. One report for the batch.
pub fn run_all(lang: &str, exe: &str, source: &str, inputs: &[String]) -> RunReport {
    let dir = run_dir();
    let _ = std::fs::create_dir_all(&dir);
    let run = |program: &mut Command, input: &str| -> CaseResult {
        let start = Instant::now();
        let mut child = match program
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return CaseResult {
                    verdict: CaseVerdict::RuntimeError,
                    actual: String::new(),
                    stderr: format!("failed to launch: {e}"),
                    ms: 0,
                }
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        let deadline = Duration::from_millis(RUN_TIMEOUT_MS);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let ms = start.elapsed().as_millis();
                    let mut out = String::new();
                    let mut err = String::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        use std::io::Read;
                        let _ = stdout.read_to_string(&mut out);
                    }
                    if let Some(mut stderr) = child.stderr.take() {
                        use std::io::Read;
                        let _ = stderr.read_to_string(&mut err);
                    }
                    // Truncate runaway output (a runaway loop printing forever).
                    out.truncate(64_000);
                    err.truncate(8_000);
                    return CaseResult {
                        verdict: if status.success() {
                            CaseVerdict::Pass // judged against expected by caller
                        } else {
                            CaseVerdict::RuntimeError
                        },
                        actual: out,
                        stderr: err,
                        ms,
                    };
                }
                Ok(None) => {
                    if start.elapsed() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return CaseResult {
                            verdict: CaseVerdict::TimeLimit,
                            actual: String::new(),
                            stderr: format!("exceeded {} ms", RUN_TIMEOUT_MS),
                            ms: start.elapsed().as_millis(),
                        };
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    return CaseResult {
                        verdict: CaseVerdict::RuntimeError,
                        actual: String::new(),
                        stderr: format!("wait error: {e}"),
                        ms: start.elapsed().as_millis(),
                    }
                }
            }
        }
    };

    let mut rep = RunReport::default();
    if lang == LANG_CPP {
        let src = dir.join("sol.cpp");
        let bin = dir.join(format!("sol{}", std::env::consts::EXE_SUFFIX));
        if std::fs::write(&src, source).is_err() {
            rep.compile_error = Some("cannot write temp source file".into());
            return rep;
        }
        let out = Command::new(exe)
            .arg("-O2")
            .arg("-std=c++17")
            .arg("-o")
            .arg(&bin)
            .arg(&src)
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let mut err = String::from_utf8_lossy(&o.stderr).to_string();
                err.truncate(8_000);
                rep.compile_error = Some(err);
                return rep;
            }
            Err(e) => {
                rep.compile_error = Some(format!("compiler launch failed: {e}"));
                return rep;
            }
        }
        for input in inputs {
            let mut cmd = Command::new(&bin);
            let mut r = run(&mut cmd, input);
            if r.verdict == CaseVerdict::Pass {
                // `actual` is compared by the caller via `judge`.
                let _ = &mut r;
            }
            rep.cases.push(r);
        }
    } else {
        let src = dir.join("sol.py");
        if std::fs::write(&src, source).is_err() {
            rep.compile_error = Some("cannot write temp source file".into());
            return rep;
        }
        for input in inputs {
            let mut cmd = Command::new(exe);
            cmd.arg("-X").arg("utf8").arg(&src);
            rep.cases.push(run(&mut cmd, input));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    rep
}

// ---------------------------------------------------------------------------
// Sample-test scraping (pure HTML parsing, no deps)
// ---------------------------------------------------------------------------

fn find_tag_start(html: &str, from: usize, tag: &str) -> Option<usize> {
    // Finds `<tag` where the next char is a delimiter (space, `>`, `/`, newline).
    let mut i = from;
    let bytes = html.as_bytes();
    while i + 1 + tag.len() <= bytes.len() {
        if bytes[i] == b'<'
            && html[i + 1..].starts_with(tag)
            && matches!(
                html[i + 1 + tag.len()..].chars().next(),
                Some(' ') | Some('>') | Some('/') | Some('\t') | Some('\n') | Some('\r')
            )
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Extract `(input, expected)` pairs from a Codeforces problem page.
/// Looks for `<div class="sample-test">`, collects its `<pre>` blocks in
/// order and pairs them (CF always alternates input/output).
pub fn parse_samples(html: &str) -> Vec<crate::model::Sample> {
    use crate::model::Sample;
    let anchor = match html.find("sample-test") {
        Some(i) => i,
        None => return Vec::new(),
    };
    // Start of the enclosing `<div ... sample-test ...>`.
    let mut start = anchor;
    while start > 0 && !html[start..].starts_with("<div") {
        start -= 1;
    }
    let bytes = html.as_bytes();
    let mut i = start;
    let mut depth = 0i32;
    let mut pres: Vec<String> = Vec::new();
    while i < bytes.len() {
        if html[i..].starts_with("</div") {
            depth -= 1;
            if depth <= 0 {
                break;
            }
            i += 6;
        } else if html[i..].starts_with("<div")
            && matches!(
                html[i + 4..].chars().next(),
                Some(' ') | Some('>') | Some('/') | Some('\t') | Some('\n') | Some('\r')
            )
        {
            depth += 1;
            i += 4;
        } else if html[i..].starts_with("<pre")
            && matches!(
                html[i + 4..].chars().next(),
                Some(' ') | Some('>') | Some('/') | Some('\t') | Some('\n') | Some('\r')
            )
        {
            let body_start = match html[i..].find('>') {
                Some(k) => i + k + 1,
                None => break,
            };
            let body_end = match html[body_start..].find("</pre>") {
                Some(k) => body_start + k,
                None => break,
            };
            pres.push(html_to_text(&html[body_start..body_end]));
            i = body_end + 6;
        } else {
            i += 1;
        }
    }
    pres.chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                Some(Sample { input: c[0].clone(), expected: c[1].clone() })
            } else {
                None
            }
        })
        .collect()
}

/// Convert `<pre>` inner HTML to plain text: `<br>` → newline, strip all
/// other tags, decode the entities Codeforces actually emits.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = html[i..].find('>') {
                let tag = html[i + 1..i + end].trim().to_ascii_lowercase();
                if tag == "br" || tag == "br/" || tag == "br /" {
                    out.push('\n');
                }
                i += end + 1;
            } else {
                break;
            }
        } else if bytes[i] == b'&' {
            if let Some(end) = html[i..].find(';') {
                if end <= 8 {
                    match &html[i..i + end + 1] {
                        "&lt;" => out.push('<'),
                        "&gt;" => out.push('>'),
                        "&amp;" => out.push('&'),
                        "&quot;" => out.push('"'),
                        "&#39;" => out.push('\''),
                        "&nbsp;" => out.push(' '),
                        other => out.push_str(other),
                    }
                    i += end + 1;
                } else {
                    out.push('&');
                    i += 1;
                }
            } else {
                out.push('&');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<div class="problem-statement"><div class="sample-tests"><div class="sample-test"><div class="input"><div class="title">Input</div><pre>8<br /></pre></div><div class="output"><div class="title">Output</div><pre>YES<br /></pre></div></div></div><div class="note">x</div></div>"#;

    const PAGE2: &str = r#"<div class="sample-test"><div class="input"><pre>4<br />1 2 3 4<br /></pre></div><div class="output"><pre>0<br /></pre></div><div class="input"><pre>1<br />5<br /></pre></div><div class="output"><pre>1<br /></pre></div></div>"#;

    #[test]
    fn parses_single_sample() {
        let s = parse_samples(PAGE);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].input.trim(), "8");
        assert_eq!(s[0].expected.trim(), "YES");
    }

    #[test]
    fn parses_multiple_samples_in_order() {
        let s = parse_samples(PAGE2);
        assert_eq!(s.len(), 2);
        assert_eq!(normalize(&s[0].input), "4\n1 2 3 4");
        assert_eq!(normalize(&s[1].expected), "1");
    }

    #[test]
    fn no_sample_section_yields_none() {
        assert!(parse_samples("<html><body>no tests</body></html>").is_empty());
    }

    #[test]
    fn entities_and_tags_decode() {
        assert_eq!(html_to_text("a &lt; b<br />&amp;").trim(), "a < b\n&");
    }

    #[test]
    fn judge_ignores_trailing_whitespace() {
        assert!(judge("YES \n", "YES"));
        assert!(judge("1 2 3", "1 2 3\n"));
        assert!(!judge("YES", "NO"));
    }
}
