//! Pack loading/validation + the embedded CP-31 dataset.

use crate::model::{Pack, Problem, ProblemKey, Sheet};
use serde::Deserialize;

/// Vendored CP-31 sheet (TLE Eliminators), 372 problems, ratings 800–1900.
const EMBEDDED_CP31_JSON: &str = include_str!("../assets/cp31.json");

#[derive(Deserialize)]
struct PackFile {
    #[allow(dead_code)]
    #[serde(default)]
    format_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    sheets: Vec<SheetFile>,
}

#[derive(Deserialize)]
struct SheetFile {
    rating: i64,
    #[serde(default)]
    problems: Vec<ProblemFile>,
}

#[derive(Deserialize)]
struct ProblemFile {
    contest_id: i64,
    index: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "cf_rating")]
    rating: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parses and normalizes a pack document. Duplicate keys are dropped (first wins),
/// sheets/problems sorted deterministically.
pub fn parse_pack(text: &str) -> Result<Pack, String> {
    let f: PackFile =
        serde_json::from_str(text).map_err(|e| format!("invalid pack JSON: {e}"))?;
    let mut sheets = Vec::with_capacity(f.sheets.len());
    for sf in f.sheets {
        let mut problems = Vec::with_capacity(sf.problems.len());
        for pf in sf.problems {
            let name = if pf.name.is_empty() {
                format!("{} / {}", pf.contest_id, pf.index)
            } else {
                pf.name
            };
            problems.push(Problem {
                key: ProblemKey::new(pf.contest_id, pf.index),
                name,
                rating: pf.rating.unwrap_or(sf.rating),
                tags: pf.tags,
            });
        }
        problems.sort_by_key(|p| (p.key.contest_id, p.key.index.clone()));
        problems.dedup_by_key(|p| p.key.clone());
        sheets.push(Sheet { rating: sf.rating, problems });
    }
    sheets.sort_by_key(|s| s.rating);
    Ok(Pack { id: f.id, name: f.name, source: f.source, sheets })
}

/// The bundled CP-31 pack. Panics only if the vendored asset itself is malformed.
pub fn embedded_cp31() -> Pack {
    parse_pack(EMBEDDED_CP31_JSON).expect("embedded cp31.json must parse")
}
