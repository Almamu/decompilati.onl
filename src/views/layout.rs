use std::sync::OnceLock;

use axum::http::StatusCode;
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::assets;

pub const SITE_NAME: &str = "decompilati.onl";

static BASE_URL: OnceLock<String> = OnceLock::new();

/// Set once at startup; used for canonical URLs and other absolute links in page heads.
pub fn set_base_url(url: &str) {
    let _ = BASE_URL.set(url.to_string());
}

pub fn base_url() -> &'static str {
    BASE_URL.get().map(String::as_str).unwrap_or("")
}

#[derive(Default)]
pub struct Page<'a> {
    pub title: &'a str,
    pub description: &'a str,
    /// Absolute URL of a preview image for social cards.
    pub image: Option<String>,
    pub body: Markup,
    /// Path of the canonical URL (e.g. `/projects/sm64`); `None` for pages that shouldn't be indexed.
    pub canonical: Option<String>,
    /// schema.org JSON-LD describing the page.
    pub json_ld: Option<serde_json::Value>,
    /// Path of a page-specific RSS feed, announced next to the site-wide one.
    pub feed: Option<String>,
    /// Open Graph type: `website` by default.
    pub og_type: Option<&'static str>,
}

/// Renders a full HTML document.
///
/// Public pages are cached at the edge and shared by every visitor, so the shell never contains
/// per-user content: the login/user area is filled in by `app.js` from `/api/me`.
pub fn render(p: Page) -> String {
    let full_title = if p.title.is_empty() { format!("{SITE_NAME} · game decompilation progress") } else { format!("{} · {SITE_NAME}", p.title) };
    let canonical = p.canonical.as_ref().map(|path| format!("{}{path}", base_url()));
    // JSON-LD is embedded in a script tag; escaping "<" keeps it from closing the tag early.
    let json_ld = p.json_ld.as_ref().map(|v| v.to_string().replace('<', "\\u003c"));
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (full_title) }
                meta name="description" content=(p.description);
                @if let Some(url) = &canonical {
                    link rel="canonical" href=(url);
                    meta property="og:url" content=(url);
                } @else {
                    meta name="robots" content="noindex";
                }
                meta property="og:site_name" content=(SITE_NAME);
                meta property="og:type" content=(p.og_type.unwrap_or("website"));
                meta property="og:title" content=(full_title);
                meta property="og:description" content=(p.description);
                meta name="twitter:title" content=(full_title);
                meta name="twitter:description" content=(p.description);
                @if let Some(img) = &p.image {
                    meta property="og:image" content=(img);
                    meta property="og:image:width" content="640";
                    meta property="og:image:height" content="360";
                    meta name="twitter:image" content=(img);
                    meta name="twitter:card" content="summary_large_image";
                } @else {
                    meta name="twitter:card" content="summary";
                }
                meta name="theme-color" content="#0d1016";
                @if let Some(ld) = &json_ld {
                    script type="application/ld+json" { (PreEscaped(ld)) }
                }
                meta name="color-scheme" content="dark light";
                link rel="icon" href=(assets::url("favicon.svg")) type="image/svg+xml";
                link rel="stylesheet" href=(assets::url("app.css"));
                link rel="alternate" type="application/rss+xml" title="decompilati.onl: all progress updates" href="/feed.xml";
                @if let Some(feed) = &p.feed {
                    link rel="alternate" type="application/rss+xml" title={ (p.title) " progress updates" } href=(feed);
                }
                script src=(assets::url("app.js")) defer {}
            }
            body {
                header.site {
                    div.wrap {
                        a.logo href="/" { span.logo-mark { "▚" } " decompilati" span.dim { ".onl" } }
                        nav {
                            a href="/platforms" { "Platforms" }
                            a href="/docs" { "Docs" }
                            span #auth {
                                a.btn.btn-small href="/auth/github" { "Log in with GitHub" }
                            }
                        }
                    }
                }
                main.wrap { (p.body) }
                footer.site {
                    div.wrap {
                        p {
                            "Progress is reported and managed by each project. "
                            "Game names and images belong to their respective owners."
                        }
                        p {
                            a href="/docs/decomp-dev" { "Coming from decomp.dev?" }
                            " · "
                            a href="/docs" { "Docs" }
                            " · "
                            a href="https://github.com/Almamu/decompilati.onl" rel="noopener" { "Source code on GitHub" }
                            " · GPL-3.0"
                        }
                    }
                }
            }
        }
    }
    .into_string()
}

pub fn error_page(status: StatusCode, msg: &str) -> String {
    render(Page {
        title: status.canonical_reason().unwrap_or("Error"),
        description: msg,
        body: html! {
            section.narrow.error {
                h1 { (status.as_u16()) " " (status.canonical_reason().unwrap_or("")) }
                p { (msg) }
                p { a.btn href="/" { "Back to the homepage" } }
            }
        },
        ..Default::default()
    })
}

/// A `<time>` element; `app.js` rewrites its text as a relative time ("3 hours ago").
pub fn time_ago(ts: i64) -> Markup {
    html! {
        time.ago datetime=(crate::util::iso_time(ts)) title=(crate::util::human_date(ts)) { (crate::util::human_date(ts)) }
    }
}

