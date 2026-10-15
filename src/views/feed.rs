//! RSS 2.0 feeds of progress reports: the whole site and one per project.

use std::fmt::Write;

use crate::{
    db::FeedItem,
    github::is_commit_hash,
    util::{fmt_delta, fmt_pct},
};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn rfc2822(ts: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc2822).ok())
        .unwrap_or_default()
}

fn item(base: &str, it: &FeedItem) -> String {
    let name = if it.version.is_empty() { it.name.clone() } else { format!("{} ({})", it.name, it.version) };
    let decompiled = it.code.decompiled_pct();
    let mut title = format!("{name}: {} decompiled", fmt_pct(decompiled));
    if let Some(prev) = &it.prev_code {
        let _ = write!(title, " ({})", fmt_delta(decompiled - prev.decompiled_pct()));
    }

    // The description is HTML, escaped once more for the XML around it.
    let mut html = format!(
        "<p>Matching {} · partially matching {}",
        fmt_pct(it.code.matching_pct()),
        fmt_pct(it.code.non_matching_pct())
    );
    if it.code.linked > 0 {
        let _ = write!(html, " · fully linked {}", fmt_pct(it.code.linked_pct()));
    }
    if it.data.total > 0 {
        let _ = write!(html, " · data {}", fmt_pct(it.data.pct()));
    }
    html.push_str("</p>");
    if let Some(m) = &it.message {
        let _ = write!(html, "<p>{}</p>", esc(m));
    }
    let commit_url = it
        .commit
        .as_deref()
        .filter(|c| is_commit_hash(c))
        .and_then(|c| crate::db::commit_link(&it.repo_host, &it.repo, it.repo_url.as_deref(), c).map(|link| (c, link)));
    if let Some((c, link)) = commit_url {
        let _ = write!(
            html,
            r#"<p><a href="{}">Commit {}</a></p>"#,
            esc(&link),
            &c[..7.min(c.len())]
        );
    }

    let link = if it.version.is_empty() {
        format!("{base}/projects/{}", it.slug)
    } else {
        format!("{base}/projects/{}/versions/{}", it.slug, it.version)
    };
    format!(
        "<item><title>{}</title><link>{}</link><guid isPermaLink=\"false\">{}-report-{}</guid><pubDate>{}</pubDate><description>{}</description></item>",
        esc(&title),
        esc(&link),
        esc(base),
        it.report_id,
        rfc2822(it.created_at),
        esc(&html),
    )
}

/// A complete feed. `path` is the feed's own path and `page` the page it describes.
pub fn rss(base: &str, title: &str, description: &str, page: &str, path: &str, items: &[FeedItem]) -> String {
    let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom"><channel>"#);
    let _ = write!(
        xml,
        r#"<title>{}</title><link>{}{}</link><description>{}</description><language>en</language><atom:link href="{}{}" rel="self" type="application/rss+xml"/>"#,
        esc(title),
        esc(base),
        esc(page),
        esc(description),
        esc(base),
        esc(path),
    );
    if let Some(first) = items.first() {
        let _ = write!(xml, "<lastBuildDate>{}</lastBuildDate>", rfc2822(first.created_at));
    }
    for it in items {
        xml.push_str(&item(base, it));
    }
    xml.push_str("</channel></rss>");
    xml
}
