//! Progress report format accepted by the webhook, plus validation/normalisation.

use serde::{Deserialize, Serialize};

pub const MAX_AREAS: usize = 2000;
pub const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeStats {
    pub total: u64,
    #[serde(default)]
    pub matching: u64,
    #[serde(default, alias = "nonmatching", alias = "non_matching_decompiled")]
    pub non_matching: u64,
}

impl CodeStats {
    pub fn add(&mut self, o: &CodeStats) {
        self.total += o.total;
        self.matching += o.matching;
        self.non_matching += o.non_matching;
    }
    pub fn matching_pct(&self) -> f64 {
        pct(self.matching, self.total)
    }
    pub fn non_matching_pct(&self) -> f64 {
        pct(self.non_matching, self.total)
    }
    /// Matching + non-matching (i.e. everything that's been decompiled in some form).
    pub fn decompiled_pct(&self) -> f64 {
        pct(self.matching + self.non_matching, self.total)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStats {
    pub total: u64,
    #[serde(default, alias = "decompiled", alias = "matching")]
    pub done: u64,
}

impl DataStats {
    pub fn add(&mut self, o: &DataStats) {
        self.total += o.total;
        self.done += o.done;
    }
    pub fn pct(&self) -> f64 {
        pct(self.done, self.total)
    }
}

/// Function counts; `total == 0` means the project doesn't report them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionStats {
    pub total: u64,
    #[serde(default, alias = "matched")]
    pub matching: u64,
}

impl FunctionStats {
    pub fn pct(&self) -> f64 {
        pct(self.matching, self.total)
    }
}

pub fn pct(part: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { (part as f64 / total as f64 * 100.0).clamp(0.0, 100.0) }
}

/// Body of `POST /api/v1/progress`.
#[derive(Debug, Deserialize)]
pub struct ReportInput {
    /// Game version this report is for (e.g. `us`, `europe`); empty for single-version projects.
    #[serde(default)]
    pub version: Option<String>,
    pub unit: Option<String>,
    pub code: Option<CodeStats>,
    pub data: Option<DataStats>,
    /// Optional function counts, shown on the project page and the functions badge.
    pub functions: Option<FunctionStats>,
    #[serde(default)]
    pub areas: Vec<AreaInput>,
    pub layout: Option<String>,
    pub commit: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AreaInput {
    pub name: String,
    pub description: Option<String>,
    pub code: Option<CodeStats>,
    pub data: Option<DataStats>,
    /// Relative tile size in the map; defaults to the code total.
    pub weight: Option<f64>,
    #[serde(default)]
    pub children: Vec<AreaInput>,
}

/// Normalised area, stored as JSON in the database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Area {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub code: CodeStats,
    pub data: DataStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Area>,
}

impl Area {
    pub fn size(&self) -> f64 {
        self.weight.unwrap_or(self.code.total.max(self.data.total) as f64).max(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Treemap,
    Grid,
}

impl Layout {
    pub fn as_str(&self) -> &'static str {
        match self {
            Layout::Treemap => "treemap",
            Layout::Grid => "grid",
        }
    }
    pub fn parse(s: &str) -> Layout {
        if s == "grid" { Layout::Grid } else { Layout::Treemap }
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub version: String,
    pub unit: String,
    pub code: CodeStats,
    pub data: DataStats,
    pub functions: FunctionStats,
    pub areas: Vec<Area>,
    pub layout: Layout,
    pub commit: Option<String>,
    pub message: Option<String>,
}

fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

fn check_code(c: &CodeStats, what: &str) -> Result<(), String> {
    if c.matching.saturating_add(c.non_matching) > c.total {
        return Err(format!("{what}: matching + non_matching ({}) exceeds total ({})", c.matching + c.non_matching, c.total));
    }
    Ok(())
}

fn check_data(d: &DataStats, what: &str) -> Result<(), String> {
    if d.done > d.total {
        return Err(format!("{what}: data.done ({}) exceeds data.total ({})", d.done, d.total));
    }
    Ok(())
}

fn normalize_area(a: AreaInput, depth: usize, count: &mut usize, path: &str) -> Result<Area, String> {
    *count += 1;
    if *count > MAX_AREAS {
        return Err(format!("too many areas (max {MAX_AREAS})"));
    }
    if depth > MAX_DEPTH {
        return Err(format!("areas nested too deeply (max depth {MAX_DEPTH})"));
    }
    let name = clean_text(&a.name, 80);
    if name.is_empty() {
        return Err(format!("{path}: area name must not be empty"));
    }
    let here = format!("{path}/{name}");
    let children = a
        .children
        .into_iter()
        .map(|c| normalize_area(c, depth + 1, count, &here))
        .collect::<Result<Vec<_>, _>>()?;
    let code = match a.code {
        Some(c) => c,
        None => children.iter().fold(CodeStats::default(), |mut acc, c| { acc.add(&c.code); acc }),
    };
    let data = match a.data {
        Some(d) => d,
        None => children.iter().fold(DataStats::default(), |mut acc, c| { acc.add(&c.data); acc }),
    };
    check_code(&code, &here)?;
    check_data(&data, &here)?;
    if let Some(w) = a.weight
        && (!w.is_finite() || w < 0.0) {
            return Err(format!("{here}: weight must be a non-negative number"));
        }
    Ok(Area {
        name,
        description: a.description.map(|d| clean_text(&d, 300)).filter(|d| !d.is_empty()),
        code,
        data,
        weight: a.weight,
        children,
    })
}

impl ReportInput {
    pub fn normalize(self) -> Result<Report, String> {
        let mut count = 0;
        let areas = self
            .areas
            .into_iter()
            .map(|a| normalize_area(a, 1, &mut count, ""))
            .collect::<Result<Vec<_>, _>>()?;
        if self.code.is_none() && areas.is_empty() {
            return Err("report must contain `code` totals and/or `areas`".into());
        }
        let code = self.code.unwrap_or_else(|| {
            areas.iter().fold(CodeStats::default(), |mut acc, a| { acc.add(&a.code); acc })
        });
        let data = self.data.unwrap_or_else(|| {
            areas.iter().fold(DataStats::default(), |mut acc, a| { acc.add(&a.data); acc })
        });
        check_code(&code, "code")?;
        check_data(&data, "data")?;
        let unit = self.unit.map(|u| clean_text(&u, 16)).filter(|u| !u.is_empty()).unwrap_or_else(|| "bytes".into());
        let version = self.version.map(|v| v.trim().to_string()).unwrap_or_default();
        if !valid_version(&version) {
            return Err("version must be up to 32 letters, digits, dots, dashes or underscores".into());
        }
        let functions = self.functions.unwrap_or_default();
        if functions.matching > functions.total {
            return Err(format!("functions.matching ({}) exceeds functions.total ({})", functions.matching, functions.total));
        }
        Ok(Report {
            version,
            unit,
            code,
            data,
            functions,
            areas,
            layout: self.layout.as_deref().map(Layout::parse).unwrap_or_default(),
            commit: self.commit.map(|c| clean_text(&c, 64)).filter(|c| !c.is_empty()),
            message: self.message.map(|m| clean_text(m.lines().next().unwrap_or(""), 200)).filter(|m| !m.is_empty()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_up_children() {
        let input: ReportInput = serde_json::from_str(
            r#"{"areas":[{"name":"Overlay","children":[
                {"name":"a","code":{"total":100,"matching":50,"non_matching":10},"data":{"total":10,"done":5}},
                {"name":"b","code":{"total":100,"matching":0,"non_matching":0}}]}]}"#,
        )
        .unwrap();
        let r = input.normalize().unwrap();
        assert_eq!(r.code, CodeStats { total: 200, matching: 50, non_matching: 10 });
        assert_eq!(r.data, DataStats { total: 10, done: 5 });
        assert_eq!(r.areas[0].code.total, 200);
    }

    #[test]
    fn rejects_overflow() {
        let input: ReportInput =
            serde_json::from_str(r#"{"code":{"total":10,"matching":8,"non_matching":5}}"#).unwrap();
        assert!(input.normalize().is_err());
    }

    #[test]
    fn rejects_empty() {
        let input: ReportInput = serde_json::from_str(r#"{}"#).unwrap();
        assert!(input.normalize().is_err());
    }
}

/// Version names come from artifact names like `europe_report`, so they stay simple.
pub fn valid_version(v: &str) -> bool {
    v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
