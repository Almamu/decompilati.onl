//! Server-rendered SVG: area treemap, progress history chart and README badge.
//! No client-side charting library is needed, which keeps pages small and cacheable.

use std::fmt::Write;

use crate::{
    db::ReportRow,
    report::Area,
    util::{fmt_count, fmt_pct, human_date},
};

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let max = row.iter().cloned().fold(f64::MIN, f64::max);
    let min = row.iter().cloned().fold(f64::MAX, f64::min);
    let s2 = side * side;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

/// Squarified treemap (Bruls et al.). `values` must be sorted descending and positive.
fn squarify(values: &[f64], mut r: Rect) -> Vec<Rect> {
    let total: f64 = values.iter().sum();
    if total <= 0.0 {
        return vec![];
    }
    let scale = r.w * r.h / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();
    let mut out = Vec::with_capacity(areas.len());
    let mut i = 0;
    while i < areas.len() {
        let side = r.w.min(r.h);
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..=j], side) <= worst(&areas[i..j], side) {
            j += 1;
        }
        let row = &areas[i..j];
        let sum: f64 = row.iter().sum();
        if r.w >= r.h {
            let cw = if r.h > 0.0 { sum / r.h } else { 0.0 };
            let mut y = r.y;
            for a in row {
                let h = if cw > 0.0 { a / cw } else { 0.0 };
                out.push(Rect { x: r.x, y, w: cw, h });
                y += h;
            }
            r.x += cw;
            r.w -= cw;
        } else {
            let rh = if r.w > 0.0 { sum / r.w } else { 0.0 };
            let mut x = r.x;
            for a in row {
                let w = if rh > 0.0 { a / rh } else { 0.0 };
                out.push(Rect { x, y: r.y, w, h: rh });
                x += w;
            }
            r.y += rh;
            r.h -= rh;
        }
        i = j;
    }
    out
}

fn fit_text(s: &str, width: f64, char_w: f64) -> Option<String> {
    let max = ((width - 8.0) / char_w).floor() as usize;
    if max < 3 {
        return None;
    }
    let n = s.chars().count();
    Some(if n <= max { s.to_string() } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) })
}

fn area_tooltip(a: &Area, unit: &str) -> String {
    let mut t = format!(
        "{}\nCode: {} matching, {} non-matching ({} {unit})",
        a.name,
        fmt_pct(a.code.matching_pct()),
        fmt_pct(a.code.non_matching_pct()),
        fmt_count(a.code.total)
    );
    if a.data.total > 0 {
        let _ = write!(t, "\nData: {} ({} {unit})", fmt_pct(a.data.pct()), fmt_count(a.data.total));
    }
    if let Some(d) = &a.description {
        let _ = write!(t, "\n{d}");
    }
    t
}

fn draw_areas(out: &mut String, areas: &[Area], r: Rect, unit: &str, depth: usize) {
    let mut items: Vec<&Area> = areas.iter().filter(|a| a.size() > 0.0).collect();
    items.sort_by(|a, b| b.size().total_cmp(&a.size()));
    let values: Vec<f64> = items.iter().map(|a| a.size()).collect();
    for (a, rect) in items.into_iter().zip(squarify(&values, r)) {
        draw_area(out, a, rect, unit, depth);
    }
}

fn draw_area(out: &mut String, a: &Area, r: Rect, unit: &str, depth: usize) {
    const PAD: f64 = 1.5;
    let r = Rect { x: r.x + PAD, y: r.y + PAD, w: (r.w - 2.0 * PAD).max(0.0), h: (r.h - 2.0 * PAD).max(0.0) };
    if r.w < 1.0 || r.h < 1.0 {
        return;
    }
    let _ = write!(out, "<g><title>{}</title>", esc(&area_tooltip(a, unit)));
    let nested = !a.children.is_empty() && r.w > 110.0 && r.h > 70.0 && depth < 3;
    if nested {
        // Group frame with a header strip, children laid out inside.
        let _ = write!(out, r#"<rect class="t-frame" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="3"/>"#, r.x, r.y, r.w, r.h);
        if let Some(label) = fit_text(&format!("{} · {}", a.name, fmt_pct(a.code.decompiled_pct())), r.w, 6.6) {
            let _ = write!(out, r#"<text class="t-head" x="{:.1}" y="{:.1}">{}</text>"#, r.x + 5.0, r.y + 13.0, esc(&label));
        }
        let inner = Rect { x: r.x + 2.0, y: r.y + 18.0, w: r.w - 4.0, h: r.h - 20.0 };
        draw_areas(out, &a.children, inner, unit, depth + 1);
    } else {
        let _ = write!(out, r#"<rect class="t-bg" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="2"/>"#, r.x, r.y, r.w, r.h);
        // "Water level" fill from the bottom: matching, then non-matching on top.
        let (m, n, class) = if a.code.total > 0 {
            (a.code.matching_pct() / 100.0, a.code.non_matching_pct() / 100.0, "t-match")
        } else {
            (a.data.pct() / 100.0, 0.0, "t-data")
        };
        let mh = r.h * m;
        let nh = r.h * n;
        if mh > 0.0 {
            let _ = write!(out, r#"<rect class="{class}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#, r.x, r.y + r.h - mh, r.w, mh);
        }
        if nh > 0.0 {
            let _ = write!(out, r#"<rect class="t-nonmatch" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#, r.x, r.y + r.h - mh - nh, r.w, nh);
        }
        if r.h > 30.0
            && let Some(label) = fit_text(&a.name, r.w, 7.2) {
                let _ = write!(out, r#"<text class="t-label" x="{:.1}" y="{:.1}">{}</text>"#, r.x + 5.0, r.y + 15.0, esc(&label));
                let pct = if a.code.total > 0 { a.code.decompiled_pct() } else { a.data.pct() };
                let _ = write!(out, r#"<text class="t-pct" x="{:.1}" y="{:.1}">{}</text>"#, r.x + 5.0, r.y + 29.0, fmt_pct(pct));
            }
    }
    out.push_str("</g>");
}

pub fn treemap(areas: &[Area], unit: &str) -> String {
    const W: f64 = 1000.0;
    const H: f64 = 560.0;
    let mut out = format!(
        r#"<svg class="treemap" viewBox="0 0 {W} {H}" role="img" aria-label="Map of project areas coloured by progress" xmlns="http://www.w3.org/2000/svg">"#
    );
    draw_areas(&mut out, areas, Rect { x: 0.0, y: 0.0, w: W, h: H }, unit, 1);
    out.push_str("</svg>");
    out
}

/// Line chart of matching %, decompiled % and data % over time.
pub fn history(rows: &[ReportRow]) -> String {
    const W: f64 = 800.0;
    const H: f64 = 240.0;
    const L: f64 = 40.0;
    const R: f64 = 10.0;
    const T: f64 = 10.0;
    const B: f64 = 24.0;
    let (t0, t1) = (rows.first().map(|r| r.created_at).unwrap_or(0), rows.last().map(|r| r.created_at).unwrap_or(1));
    let span = ((t1 - t0) as f64).max(1.0);
    let x = |t: i64| L + (t - t0) as f64 / span * (W - L - R);
    let y = |v: f64| T + (100.0 - v) / 100.0 * (H - T - B);

    let mut out = format!(
        r#"<svg class="history" viewBox="0 0 {W} {H}" role="img" aria-label="Progress over time" xmlns="http://www.w3.org/2000/svg">"#
    );
    for v in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let _ = write!(
            out,
            r#"<line class="h-grid" x1="{L}" x2="{}" y1="{:.1}" y2="{:.1}"/><text class="h-axis" x="{}" y="{:.1}" text-anchor="end">{v}%</text>"#,
            W - R, y(v), y(v), L - 6.0, y(v) + 4.0
        );
    }
    let _ = write!(out, r#"<text class="h-axis" x="{L}" y="{}">{}</text>"#, H - 6.0, &human_date(t0)[..10]);
    let _ = write!(out, r#"<text class="h-axis" x="{}" y="{}" text-anchor="end">{}</text>"#, W - R, H - 6.0, &human_date(t1)[..10]);

    let has_data = rows.iter().any(|r| r.data.total > 0);
    let series: [(&str, &dyn Fn(&ReportRow) -> f64); 3] = [
        ("h-decomp", &|r: &ReportRow| r.code.decompiled_pct()),
        ("h-match", &|r: &ReportRow| r.code.matching_pct()),
        ("h-data", &|r: &ReportRow| r.data.pct()),
    ];
    for (class, f) in series {
        if class == "h-data" && !has_data {
            continue;
        }
        // Step line: progress holds until the next report.
        let mut pts = String::new();
        let mut prev: Option<f64> = None;
        for r in rows {
            let v = f(r);
            if let Some(p) = prev {
                let _ = write!(pts, "{:.1},{:.1} ", x(r.created_at), y(p));
            }
            let _ = write!(pts, "{:.1},{:.1} ", x(r.created_at), y(v));
            prev = Some(v);
        }
        if let Some(p) = prev {
            let _ = write!(pts, "{:.1},{:.1}", W - R, y(p));
        }
        let _ = write!(out, r#"<polyline class="{class}" points="{pts}"/>"#);
    }
    out.push_str("</svg>");
    out
}

/// Shields-style badge. `pct` picks the colour; `None` draws a grey "not reported" badge.
pub fn badge(label: &str, value: &str, pct: Option<f64>) -> String {
    let color = match pct {
        None => "#9f9f9f",
        Some(p) if p >= 100.0 => "#2ea043",
        Some(p) if p >= 75.0 => "#57ab5a",
        Some(p) if p >= 50.0 => "#c69026",
        Some(p) if p >= 25.0 => "#d4762c",
        Some(_) => "#c94c4c",
    };
    let value = esc(value);
    let lw = 10.0 + label.chars().count() as f64 * 6.5;
    let vw = 10.0 + value.chars().count() as f64 * 7.0;
    let w = lw + vw;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="20" role="img" aria-label="{l}: {value}"><title>{l}: {value}</title><linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/><stop offset="1" stop-opacity=".1"/></linearGradient><clipPath id="r"><rect width="{w:.0}" height="20" rx="3" fill="#fff"/></clipPath><g clip-path="url(#r)"><rect width="{lw:.0}" height="20" fill="#555"/><rect x="{lw:.0}" width="{vw:.0}" height="20" fill="{color}"/><rect width="{w:.0}" height="20" fill="url(#s)"/></g><g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11"><text x="{lx:.1}" y="14">{l}</text><text x="{vx:.1}" y="14">{value}</text></g></svg>"##,
        l = esc(label),
        lx = lw / 2.0,
        vx = lw + vw / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_covers_area() {
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 });
        let total: f64 = rects.iter().map(|r| r.w * r.h).sum();
        assert!((total - 24.0).abs() < 1e-6);
        for r in rects {
            assert!(r.x >= -1e-9 && r.y >= -1e-9 && r.x + r.w <= 6.0 + 1e-9 && r.y + r.h <= 4.0 + 1e-9);
        }
    }
}
