use axum::http::StatusCode;
use maud::{html, Markup, DOCTYPE};

use crate::assets;

pub const SITE_NAME: &str = "decompilati.onl";

pub struct Page<'a> {
    pub title: &'a str,
    pub description: &'a str,
    /// Absolute URL of a preview image for social cards.
    pub image: Option<String>,
    pub body: Markup,
}

/// Renders a full HTML document.
///
/// Public pages are cached at the edge and shared by every visitor, so the shell never contains
/// per-user content: the login/user area is filled in by `app.js` from `/api/me`.
pub fn render(p: Page) -> String {
    let full_title = if p.title.is_empty() { SITE_NAME.to_string() } else { format!("{} · {SITE_NAME}", p.title) };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (full_title) }
                meta name="description" content=(p.description);
                meta property="og:title" content=(full_title);
                meta property="og:description" content=(p.description);
                @if let Some(img) = &p.image {
                    meta property="og:image" content=(img);
                    meta name="twitter:card" content="summary_large_image";
                }
                meta name="color-scheme" content="dark light";
                link rel="icon" href=(assets::url("favicon.svg")) type="image/svg+xml";
                link rel="stylesheet" href=(assets::url("app.css"));
                link rel="alternate" type="application/rss+xml" title="Project updates" href="/feed.xml";
                script src=(assets::url("app.js")) defer {}
            }
            body {
                header.site {
                    div.wrap {
                        a.logo href="/" { span.logo-mark { "▚" } " decompilati" span.dim { ".onl" } }
                        nav {
                            a href="/projects" { "Projects" }
                            a href="/platforms" { "Platforms" }
                            a href="/docs" { "API" }
                            span #auth {
                                a.btn.btn-small href="/auth/github" { "Log in with GitHub" }
                            }
                        }
                    }
                }
                main.wrap { (p.body) }
                footer.site {
                    div.wrap {
                        p { "Progress is self-reported by each project via the " a href="/docs" { "webhook API" } ". "
                            "Game names and images belong to their respective owners." }
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
        image: None,
        body: html! {
            section.narrow.error {
                h1 { (status.as_u16()) " " (status.canonical_reason().unwrap_or("")) }
                p { (msg) }
                p { a.btn href="/" { "Back to the homepage" } }
            }
        },
    })
}

/// A `<time>` element; `app.js` rewrites its text as a relative time ("3 hours ago").
pub fn time_ago(ts: i64) -> Markup {
    html! {
        time.ago datetime=(crate::util::iso_time(ts)) title=(crate::util::human_date(ts)) { (crate::util::human_date(ts)) }
    }
}

