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
