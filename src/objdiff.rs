//! Compatibility with objdiff's `Report` JSON (`objdiff-cli report generate`), the format
//! decomp.dev consumed. It's converted into our own report format:
//!
//! - matched code -> matching
//! - fuzzy match minus matched code -> partially matching (objdiff has no exact "decompiled but
//!   not matching" figure; the fuzzy percentage is the closest it reports)
//! - complete code (fully linked units) -> linked
//! - matched data -> data done
//! - categories (`progress_categories`) -> areas, with their units nested inside

use serde::{Deserialize, Deserializer};

use crate::report::{AreaInput, CodeStats, DataStats, FunctionStats, ReportInput, MAX_AREAS};

/// objdiff writes 64-bit integers as strings (protobuf JSON); accept both.
fn num<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        U(u64),
        F(f64),
        S(String),
    }
    Ok(match N::deserialize(d)? {
        N::U(v) => v,
        N::F(v) => v.max(0.0) as u64,
        N::S(s) => s.parse().unwrap_or(0),
    })
}

fn pct<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        F(f64),
        S(String),
    }
    Ok(match N::deserialize(d)? {
        N::F(v) => v,
        N::S(s) => s.parse().unwrap_or(0.0),
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct Measures {
    #[serde(default, deserialize_with = "pct", alias = "fuzzyMatchPercent")]
    fuzzy_match_percent: f64,
    #[serde(default, deserialize_with = "num", alias = "totalCode")]
    total_code: u64,
    #[serde(default, deserialize_with = "num", alias = "matchedCode")]
    matched_code: u64,
    #[serde(default, deserialize_with = "num", alias = "completeCode")]
    complete_code: u64,
    #[serde(default, deserialize_with = "num", alias = "totalData")]
    total_data: u64,
    #[serde(default, deserialize_with = "num", alias = "matchedData")]
    matched_data: u64,
    #[serde(default, deserialize_with = "num", alias = "totalFunctions")]
    total_functions: u64,
    #[serde(default, deserialize_with = "num", alias = "matchedFunctions")]
    matched_functions: u64,
}

impl Measures {
    /// A report with just these totals (no areas), e.g. for decomp.dev history entries.
    pub fn into_input(&self, commit: Option<String>, message: Option<String>) -> ReportInput {
        ReportInput {
            version: None,
            unit: Some("bytes".into()),
            code: Some(self.code()),
            data: Some(self.data()),
            functions: self.functions(),
            areas: vec![],
            layout: None,
            commit,
            message,
        }
    }
    fn code(&self) -> CodeStats {
        let total = self.total_code;
        let matching = self.matched_code.min(total);
        let fuzzy = ((self.fuzzy_match_percent.clamp(0.0, 100.0) / 100.0) * total as f64).round() as u64;
        CodeStats {
            total,
            matching,
            non_matching: fuzzy.saturating_sub(matching).min(total - matching),
            linked: self.complete_code.min(total),
        }
    }
    fn functions(&self) -> Option<FunctionStats> {
        (self.total_functions > 0)
            .then(|| FunctionStats { total: self.total_functions, matching: self.matched_functions.min(self.total_functions) })
    }
    fn data(&self) -> DataStats {
        DataStats { total: self.total_data, done: self.matched_data.min(self.total_data) }
    }
}

#[derive(Debug, Deserialize)]
struct Unit {
    name: String,
    #[serde(default)]
    measures: Option<Measures>,
    #[serde(default)]
    metadata: Option<UnitMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct UnitMetadata {
    #[serde(default, alias = "progressCategories")]
    progress_categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Category {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    measures: Option<Measures>,
}

#[derive(Debug, Deserialize)]
pub struct Report {
    measures: Measures,
    #[serde(default)]
    units: Vec<Unit>,
    #[serde(default)]
    categories: Vec<Category>,
}

/// True if the JSON body looks like an objdiff report rather than our own format.
pub fn looks_like_objdiff(body: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Probe {
        measures: Option<serde::de::IgnoredAny>,
        code: Option<serde::de::IgnoredAny>,
        areas: Option<serde::de::IgnoredAny>,
    }
    serde_json::from_slice::<Probe>(body).is_ok_and(|p| p.measures.is_some() && p.code.is_none() && p.areas.is_none())
}

fn area(name: &str, m: &Measures, children: Vec<AreaInput>) -> AreaInput {
    AreaInput {
        name: name.to_string(),
        description: None,
        code: Some(m.code()),
        data: Some(m.data()),
        functions: m.functions(),
        weight: None,
        children,
    }
}

/// Shortens unit names like `main/src/game/actor/foo` to something readable in the map.
fn unit_label(name: &str) -> &str {
    let trimmed = name.strip_prefix("main/").unwrap_or(name);
    if trimmed.chars().count() <= 60 {
        trimmed
    } else {
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    }
}

impl Report {
    pub fn into_input(self, commit: Option<String>, message: Option<String>) -> ReportInput {
        let units: Vec<&Unit> = self
            .units
            .iter()
            .filter(|u| u.measures.as_ref().is_some_and(|m| m.total_code + m.total_data > 0))
            .collect();
        let areas = if !self.categories.is_empty() {
            // Units appear inside each category they belong to, as long as the map stays small enough.
            let memberships: usize = units
                .iter()
                .map(|u| u.metadata.as_ref().map_or(0, |m| m.progress_categories.len()))
                .sum();
            let nest = self.categories.len() + memberships <= MAX_AREAS;
            self.categories
                .iter()
                .filter_map(|c| {
                    let m = c.measures.as_ref()?;
                    let children = if nest {
                        units
                            .iter()
                            .filter(|u| u.metadata.as_ref().is_some_and(|md| md.progress_categories.contains(&c.id)))
                            .map(|u| area(unit_label(&u.name), u.measures.as_ref().unwrap(), vec![]))
                            .collect()
                    } else {
                        vec![]
                    };
                    let name = if c.name.is_empty() { &c.id } else { &c.name };
                    Some(area(name, m, children))
                })
                .collect()
        } else if units.len() <= 300 {
            units.iter().map(|u| area(unit_label(&u.name), u.measures.as_ref().unwrap(), vec![])).collect()
        } else {
            vec![]
        };
        ReportInput {
            version: None,
            unit: Some("bytes".into()),
            code: Some(self.measures.code()),
            data: Some(self.measures.data()),
            functions: self.measures.functions(),
            areas,
            layout: None,
            commit,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_objdiff_report() {
        let body = br#"{
            "measures": {"fuzzy_match_percent": 60.0, "total_code": "1000", "matched_code": "500", "complete_code": "300", "matched_functions": 4,
                         "total_data": "200", "matched_data": "50", "total_functions": 10},
            "units": [
                {"name": "main/src/a", "measures": {"total_code": "600", "matched_code": "500", "fuzzy_match_percent": 90.0},
                 "metadata": {"progress_categories": ["game"]}},
                {"name": "main/src/b", "measures": {"total_code": "400"}, "metadata": {"progress_categories": ["game", "sdk"]}}
            ],
            "categories": [
                {"id": "game", "name": "Game", "measures": {"total_code": "1000", "matched_code": "500"}},
                {"id": "sdk", "name": "SDK", "measures": {"total_code": "400"}}
            ]
        }"#;
        assert!(looks_like_objdiff(body));
        let report: Report = serde_json::from_slice(body).unwrap();
        let r = report.into_input(Some("abc1234".into()), None).normalize().unwrap();
        assert_eq!(r.code, CodeStats { total: 1000, matching: 500, non_matching: 100, linked: 300 });
        assert_eq!(r.data, DataStats { total: 200, done: 50 });
        assert_eq!(r.functions, FunctionStats { total: 10, matching: 4 });
        assert_eq!(r.areas.len(), 2);
        assert_eq!(r.areas[0].children.len(), 2);
        assert_eq!(r.areas[0].children[0].name, "src/a");
        assert_eq!(r.areas[0].children[0].code.non_matching, 40);
        assert!(!looks_like_objdiff(br#"{"code": {"total": 1}}"#));
    }
}
