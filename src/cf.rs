//! Codeforces API client — blocking `ureq`, called only on explicit user request
//! (refresh button / first-run). No background polling anywhere.
//!
//! Parse functions are separated from IO so tests and the benchmark feed them
//! deterministic fixture payloads instead of hitting the network.

use crate::model::*;
use serde::Deserialize;
use std::time::Duration;

const API: &str = "https://codeforces.com/api";

pub struct CfClient {
    agent: ureq::Agent,
}

impl Default for CfClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CfClient {
    pub fn new() -> Self {
        // schannel on Windows / SecurityFramework on macOS / OpenSSL on Linux.
        // ureq's default TLS config is NoTlsConfig when the rustls feature is
        // disabled, so the native connector must be wired explicitly.
        let tls = std::sync::Arc::new(native_tls::TlsConnector::new().expect("tls init"));
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(25))
            .tls_connector(tls)
            .build();
        Self { agent }
    }

    fn get_json(&self, url: &str) -> Result<String, String> {
        let resp = self.agent.get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, r) => {
                format!("Codeforces API returned HTTP {code} ({})", r.get_url())
            }
            ureq::Error::Transport(t) => format!("network error: {t}"),
        })?;
        resp.into_string().map_err(|e| format!("failed to read response: {e}"))
    }

    pub fn fetch_profile(&self, handle: &str) -> Result<CfProfile, String> {
        parse_profile(&self.get_json(&format!("{API}/user.info?handles={}", enc(handle)))?)
    }

    pub fn fetch_ratings(&self, handle: &str) -> Result<Vec<RatingPoint>, String> {
        parse_ratings(&self.get_json(&format!("{API}/user.rating?handle={}", enc(handle)))?)
    }

    /// Latest `count` submissions (newest first from the API).
    pub fn fetch_submissions(&self, handle: &str, count: u32) -> Result<Vec<Submission>, String> {
        parse_submissions(&self
            .get_json(&format!("{API}/user.status?handle={}&from=1&count={count}", enc(handle)))?)
    }
}

// ---------------------------------------------------------------------------
// Authenticated web session (Codeforces has no submit API — this drives the
// same HTML form a browser submits). SESSION-ONLY BY DESIGN: cookies live in
// this struct, which lives in app memory. Nothing here is ever serialized;
// dropping it (logout / app close) destroys the session.
// ---------------------------------------------------------------------------

const WEB: &str = "https://codeforces.com";
const BFAA_FIXED: &str = "f1b3f18c715565b589b7823cda7448ce";

/// In-memory cookie jar (ureq is built without its cookie feature).
#[derive(Clone, Debug, Default)]
pub struct CookieJar {
    cookies: std::collections::BTreeMap<String, String>,
}

impl CookieJar {
    pub fn store_values(&mut self, set_cookie: &[&str]) {
        for raw in set_cookie {
            let pair = raw.split(';').next().unwrap_or("").trim();
            if let Some((k, v)) = pair.split_once('=') {
                let k = k.trim();
                if !k.is_empty() && !k.starts_with('$') {
                    self.cookies.insert(k.to_string(), v.trim().to_string());
                }
            }
        }
    }

    pub fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

/// Authenticated browser-equivalent session. Memory-only, never persisted.
pub struct WebSession {
    agent: ureq::Agent,
    jar: CookieJar,
    ftaa: String,
    bfaa: String,
    handle: String,
}

impl WebSession {
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Unauthenticated session for public pages (sample statements).
    /// Carries no identity; safe to construct and drop freely.
    pub fn anonymous() -> Self {
        let tls = std::sync::Arc::new(native_tls::TlsConnector::new().expect("tls init"));
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(25))
            .redirects(0)
            .tls_connector(tls)
            .build();
        Self {
            agent,
            jar: CookieJar::default(),
            ftaa: rand_token(18),
            bfaa: BFAA_FIXED.into(),
            handle: String::new(),
        }
    }

    /// Log in once. The password is used for this single POST and then
    /// dropped with the caller's buffer — it is never stored anywhere.
    pub fn login(handle_or_email: &str, password: &str) -> Result<Self, String> {
        let tls = std::sync::Arc::new(native_tls::TlsConnector::new().expect("tls init"));
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(25))
            .redirects(0)
            .tls_connector(tls)
            .build();
        let mut s = Self {
            agent,
            jar: CookieJar::default(),
            ftaa: rand_token(18),
            bfaa: BFAA_FIXED.into(),
            handle: String::new(),
        };
        let enter = s.get_follow(&format!("{WEB}/enter"))?;
        let csrf = find_csrf(&enter).ok_or("login page changed (no csrf token)")?;
        let ftaa = s.ftaa.clone();
        let bfaa = s.bfaa.clone();
        let body = s.post_form(
            &format!("{WEB}/enter"),
            &[
                ("csrf_token", csrf.as_str()),
                ("action", "enter"),
                ("ftaa", ftaa.as_str()),
                ("bfaa", bfaa.as_str()),
                ("handleOrEmail", handle_or_email),
                ("password", password),
                ("_tta", "176"),
                ("remember", "on"),
            ],
            Some(&csrf),
        )?;
        let handle = find_handle(&body).ok_or_else(|| {
            find_cf_error(&body)
                .map(|e| format!("login rejected: {e}"))
                .unwrap_or_else(|| {
                    "login failed — wrong handle/email or password (or a CAPTCHA is required; log in once in a browser, then retry)".into()
                })
        })?;
        s.handle = handle.clone();
        Ok(s)
    }

    /// Drop server-side validity hint; the real cleanup is dropping `self`.
    pub fn handle_name(&self) -> &str {
        &self.handle
    }

    /// Raw problem-statement HTML (public page; works with or without login,
    /// but the session's agent is reused for TLS consistency).
    pub fn fetch_problem_html(&mut self, contest_id: i64, index: &str) -> Result<String, String> {
        self.get_follow(&format!("{WEB}/problemset/problem/{contest_id}/{index}"))
    }

    /// Available `(programTypeId, label)` pairs parsed live from the contest
    /// submit page, so language IDs never go stale.
    pub fn submit_langs(&mut self, contest_id: i64) -> Result<Vec<(String, String)>, String> {
        let html = self.get_follow(&format!("{WEB}/contest/{contest_id}/submit"))?;
        find_handle(&html).ok_or("session expired — log in again")?;
        Ok(parse_lang_options(&html))
    }

    /// Submit `source` for `contest_id`/`index` as the logged-in user.
    /// Returns when Codeforces confirms receipt (not when judged).
    pub fn submit(
        &mut self,
        contest_id: i64,
        index: &str,
        program_type_id: &str,
        source: &str,
    ) -> Result<(), String> {
        let url = format!("{WEB}/contest/{contest_id}/submit");
        let page = self.get_follow(&url)?;
        find_handle(&page).ok_or("session expired — log in again")?;
        let csrf = find_csrf(&page).ok_or("submit page changed (no csrf token)")?;
        let ftaa = self.ftaa.clone();
        let bfaa = self.bfaa.clone();
        let cid = contest_id.to_string();
        let body = self.post_form(
            &format!("{url}?csrf_token={csrf}"),
            &[
                ("csrf_token", csrf.as_str()),
                ("ftaa", ftaa.as_str()),
                ("bfaa", bfaa.as_str()),
                ("action", "submitSolutionFormSubmitted"),
                ("submittedProblemIndex", index),
                ("programTypeId", program_type_id),
                ("contestId", cid.as_str()),
                ("source", source),
                ("tabSize", "4"),
                ("_tta", "594"),
                ("sourceCodeConfirmed", "true"),
            ],
            Some(&csrf),
        )?;
        if body.contains("submitted successfully") {
            return Ok(());
        }
        Err(find_cf_error(&body)
            .map(|e| format!("submit rejected: {e}"))
            .unwrap_or_else(|| "submit failed — Codeforces did not confirm receipt".into()))
    }

    // -- low-level plumbing --------------------------------------------------

    fn capture(&mut self, resp: &ureq::Response) {
        self.jar.store_values(&resp.all("set-cookie"));
    }

    fn apply_cookies(&self, req: ureq::Request) -> ureq::Request {
        if self.jar.is_empty() {
            req
        } else {
            req.set("Cookie", &self.jar.header())
        }
    }

    /// GET with manual redirect following so cookies set on 302 hops are kept.
    fn get_follow(&mut self, url: &str) -> Result<String, String> {
        let mut url = url.to_string();
        for _ in 0..6 {
            let req = self.apply_cookies(
                self.agent
                    .get(&url)
                    .set("User-Agent", "RTOM/1.0 (personal tool; contact via Codeforces handle)"),
            );
            let (status, body, location) = match req.call() {
                Ok(resp) => {
                    let status = resp.status();
                    let loc = resp.header("location").map(|s| s.to_string());
                    self.capture(&resp);
                    let text = resp.into_string().map_err(|e| format!("read error: {e}"))?;
                    (status, text, loc)
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let loc = resp.header("location").map(|s| s.to_string());
                    self.capture(&resp);
                    let text = resp.into_string().unwrap_or_default();
                    (code, text, loc)
                }
                Err(ureq::Error::Transport(t)) => return Err(format!("network error: {t}")),
            };
            if (300..400).contains(&status) {
                if let Some(loc) = location {
                    url = if loc.starts_with("http") { loc } else { format!("{WEB}{loc}") };
                    continue;
                }
            }
            if (200..300).contains(&status) {
                return Ok(body);
            }
            return Err(format!("Codeforces returned HTTP {status}"));
        }
        Err("too many redirects".into())
    }

    fn post_form(
        &mut self,
        url: &str,
        fields: &[(&str, &str)],
        csrf: Option<&str>,
    ) -> Result<String, String> {
        let mut req = self.apply_cookies(
            self.agent
                .post(url)
                .set("User-Agent", "RTOM/1.0 (personal tool; contact via Codeforces handle)")
                .set("Referer", WEB),
        );
        if let Some(c) = csrf {
            req = req.set("X-Csrf-Token", c);
        }
        match req.send_form(fields) {
            Ok(resp) => {
                let status = resp.status();
                let loc = resp.header("location").map(|s| s.to_string());
                self.capture(&resp);
                let text = resp.into_string().map_err(|e| format!("read error: {e}"))?;
                if (300..400).contains(&status) {
                    if let Some(loc) = loc {
                        let next = if loc.starts_with("http") { loc } else { format!("{WEB}{loc}") };
                        return self.get_follow(&next);
                    }
                }
                Ok(text)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let loc = resp.header("location").map(|s| s.to_string());
                self.capture(&resp);
                let text = resp.into_string().unwrap_or_default();
                if (300..400).contains(&code) {
                    if let Some(loc) = loc {
                        let next = if loc.starts_with("http") { loc } else { format!("{WEB}{loc}") };
                        return self.get_follow(&next);
                    }
                }
                Err(format!("Codeforces returned HTTP {code}"))
            }
            Err(ureq::Error::Transport(t)) => Err(format!("network error: {t}")),
        }
    }
}

fn rand_token(n: usize) -> String {
    // No rand crate in the tree; time+pid mixed is plenty for an anti-cache token.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    (std::time::SystemTime::now(), std::process::id(), n).hash(&mut h);
    let mut x = h.finish();
    const ALPH: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(n);
    for _ in 0..n {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push(ALPH[(x >> 33) as usize % ALPH.len()] as char);
    }
    out
}

/// `csrf='...'` appears in the csrf-token span and inline scripts.
pub fn find_csrf(html: &str) -> Option<String> {
    let i = html.find("csrf='")? + 6;
    let end = html[i..].find('\'')?;
    Some(html[i..i + end].to_string())
}

/// `handle = "tourist"` is embedded in every page for logged-in users.
pub fn find_handle(html: &str) -> Option<String> {
    let i = html.find("handle = \"")? + 10;
    let end = html[i..].find('"')?;
    Some(html[i..i + end].to_string())
}

/// Best-effort extraction of the red error banner text.
pub fn find_cf_error(html: &str) -> Option<String> {
    let mut i = 0;
    while let Some(k) = html[i..].find("error") {
        let j = i + k;
        // Look for `...error...">MESSAGE</span>` within a short window.
        let window = &html[j..(j + 400).min(html.len())];
        if let Some(gt) = window.find("\">") {
            let rest = &window[gt + 2..];
            if let Some(end) = rest.find("</span>") {
                let msg = crate::runner::html_to_text(&rest[..end]);
                let msg = msg.trim().to_string();
                if !msg.is_empty() && msg.len() < 300 {
                    return Some(msg);
                }
            }
        }
        i = j + 5;
    }
    None
}

/// `(programTypeId, label)` pairs from the submit page's language dropdown.
pub fn parse_lang_options(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let sel = match html.find("programTypeId") {
        Some(i) => i,
        None => return out,
    };
    let end_sel = html[sel..].find("</select>").map(|k| sel + k).unwrap_or(html.len());
    let mut i = sel;
    while let Some(k) = html[i..end_sel].find("<option") {
        let o = i + k;
        let val = html[o..end_sel]
            .find("value=\"")
            .map(|v| {
                let s = o + v + 7;
                html[s..end_sel].find('"').map(|e| html[s..s + e].to_string())
            })
            .flatten();
        let label = html[o..end_sel].find('>').map(|g| {
            let s = o + g + 1;
            html[s..end_sel]
                .find("</option>")
                .map(|e| crate::runner::html_to_text(&html[s..s + e]).trim().to_string())
                .unwrap_or_default()
        });
        if let (Some(id), Some(label)) = (val, label) {
            if !id.is_empty() && !label.is_empty() {
                out.push((id, label));
            }
        }
        i = o + 7;
    }
    out
}

/// Pick a `programTypeId` for an editor language from live dropdown options,
/// falling back to long-stable IDs when parsing yields nothing usable.
pub fn pick_program_type(lang: &str, options: &[(String, String)]) -> (String, String) {
    let want: &[&str] = if lang == crate::runner::LANG_PY {
        &["Python 3", "PyPy 3"]
    } else {
        &["GNU G++17", "G++"]
    };
    for w in want {
        if let Some(o) = options.iter().find(|(_, label)| label.contains(w)) {
            return o.clone();
        }
    }
    // Fallbacks: IDs stable on Codeforces for years (54 = GNU G++17, 31 = Python 3).
    if lang == crate::runner::LANG_PY {
        ("31".into(), "Python 3 (fallback)".into())
    } else {
        ("54".into(), "GNU G++17 (fallback)".into())
    }
}

/// Outcome of the one-shot login worker → UI thread.
pub enum AuthOutcome {
    LoggedIn { session: WebSession, handle: String },
    Failed(String),
}

/// Outcome of one-shot submit/verdict workers → UI thread. The session is
/// moved through the worker and handed back so login survives submits.
pub enum SubmitOutcome {
    Sent { at_ts: i64, session: WebSession },
    SubmitFailed { error: String, session: Option<WebSession> },
    Verdict { text: String, ok: bool },
    VerdictFailed(String),
}


/// Result of one sync run, delivered from the worker thread to the UI.
pub enum SyncOutcome {
    Done {
        profile: CfProfile,
        ratings: Vec<RatingPoint>,
        subs: Vec<Submission>,
    },
    Failed(String),
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    handle: String,
    rating: Option<i64>,
    max_rating: Option<i64>,
    rank: Option<String>,
    max_rank: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingChange {
    contest_name: String,
    rating_update_time_seconds: i64,
    old_rating: i64,
    new_rating: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusEntry {
    id: i64,
    creation_time_seconds: i64,
    problem: ProblemRef,
    verdict: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProblemRef {
    contest_id: Option<i64>,
    index: String,
    rating: Option<i64>,
}

pub fn parse_profile(json: &str) -> Result<CfProfile, String> {
    let env: Envelope<UserInfo> =
        serde_json::from_str(json).map_err(|e| format!("unexpected profile response: {e}"))?;
    let u = env.result.into_iter().next().ok_or("empty profile response")?;
    Ok(CfProfile {
        handle: u.handle,
        rating: u.rating,
        max_rating: u.max_rating,
        rank: u.rank.unwrap_or_default(),
        max_rank: u.max_rank.unwrap_or_default(),
    })
}

pub fn parse_ratings(json: &str) -> Result<Vec<RatingPoint>, String> {

    let env: Envelope<RatingChange> =
        serde_json::from_str(json).map_err(|e| format!("unexpected rating response: {e}"))?;
    Ok(env
        .result
        .into_iter()
        .map(|c| RatingPoint {
            ts: c.rating_update_time_seconds,
            rating: c.new_rating,
            delta: c.new_rating - c.old_rating,
            contest: c.contest_name,
        })
        .collect())
}

pub fn parse_submissions(json: &str) -> Result<Vec<Submission>, String> {
    let env: Envelope<StatusEntry> =
        serde_json::from_str(json).map_err(|e| format!("unexpected submissions response: {e}"))?;
    Ok(env
        .result
        .into_iter()
        .filter_map(|s| {
            // Skip gym/problemset entries without a contest id and pending submissions.
            let cid = s.problem.contest_id?;
            Some(Submission {
                id: s.id,
                ts: s.creation_time_seconds,
                key: ProblemKey::new(cid, s.problem.index),
                verdict: s.verdict?,
                rating: s.problem.rating,
            })
        })
        .collect())
}

/// Deterministic payload builders used by the benchmark and tests (no network).
pub mod fixtures {
    use crate::model::ProblemKey;
    use serde_json::json;

    pub fn profile_json(handle: &str, rating: i64) -> String {
        json!({ "status": "OK", "result": [{
            "handle": handle, "rating": rating, "maxRating": rating + 12,
            "rank": "expert", "maxRank": "expert"
        }]})
        .to_string()
    }

    /// `(key, submission id, creation timestamp)` triples → user.status envelope.
    pub fn status_json(entries: &[(ProblemKey, i64, i64)]) -> String {
        json!({ "status": "OK", "result": entries.iter().map(|(k, id, ts)| json!({
            "id": id,
            "contestId": k.contest_id,
            "index": k.index,
            "name": "fixture",
            "creationTimeSeconds": ts,
            "verdict": "OK",
            "problem": { "contestId": k.contest_id, "index": k.index, "name": "fixture" }
        })).collect::<Vec<_>>() })
        .to_string()
    }
    /// Same envelope with a configurable verdict (bench cache-stress filler).
    pub fn status_json_verdict(entries: &[(ProblemKey, i64, i64)], verdict: &str) -> String {
        json!({ "status": "OK", "result": entries.iter().map(|(k, id, ts)| json!({
            "id": id,
            "contestId": k.contest_id,
            "index": k.index,
            "name": "fixture",
            "creationTimeSeconds": ts,
            "verdict": verdict,
            "problem": { "contestId": k.contest_id, "index": k.index, "name": "fixture" }
        })).collect::<Vec<_>>() })
        .to_string()
    }
}
