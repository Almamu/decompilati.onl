use maud::{html, Markup, PreEscaped};

use crate::{
    db::Project,
    platforms,
    report::{CodeStats, DataStats},
    util::{fmt_delta, fmt_pct},
    views::layout::time_ago,
};

pub fn platform_chip(slug: &str) -> Markup {
    let p = platforms::get_or_other(slug);
    html! {
        a.chip href={ "/platform/" (p.slug) } style={ "--chip:" (p.color) } title=(p.name) { (p.short) }
    }
}

pub fn ai_badge() -> Markup {
    html! { span.badge.ai title="Self-reported: this project is mainly AI-generated" { "AI" } }
}

pub fn cover(p: &Project, eager: bool) -> Markup {
    let plat = platforms::get_or_other(&p.platform);
    html! {
        div.cover-wrap {
            @if let Some(img) = &p.image {
                img.cover src={ "/uploads/" (img) } alt={ (p.name) " cover art" } width="640" height="360"
                    loading=(if eager { "eager" } else { "lazy" }) decoding="async";
            } @else {
                div.cover.placeholder style={ "--chip:" (plat.color) } aria-hidden="true" {
                    span { (plat.short) }
                }
            }
            @if p.ai_generated {
                span.badge.ai.on-cover title="Self-reported: this project is mainly AI-generated" { "AI generated" }
            }
        }
    }
}

/// Stacked bar for code: matching / partially matching / not decompiled.
pub fn code_bar(c: &CodeStats) -> Markup {
    let m = c.matching_pct();
    let n = c.non_matching_pct();
    let label = format!("Code: {} matching, {} partially matching", fmt_pct(m), fmt_pct(n));
    html! {
        div.bar role="img" aria-label=(label) title=(label) {
            span.seg.matching style={ "width:" (format!("{m:.3}")) "%" } {}
            span.seg.nonmatching style={ "width:" (format!("{n:.3}")) "%" } {}
        }
    }
}

pub fn data_bar(d: &DataStats) -> Markup {
    let v = d.pct();
    let label = format!("Data: {}", fmt_pct(v));
    html! {
        div.bar role="img" aria-label=(label) title=(label) {
            span.seg.data style={ "width:" (format!("{v:.3}")) "%" } {}
        }
    }
}

/// Code + data progress block, compact (for cards) or full (for the project page).
pub fn progress(p: &Project, full: bool) -> Markup {
    html! {
        div.progress.full[full] {
            div.prow {
                span.plabel { "Code" }
                (code_bar(&p.code))
                span.pval { (fmt_pct(p.code.decompiled_pct())) }
            }
            @if p.data.total > 0 {
                div.prow {
                    span.plabel { "Data" }
                    (data_bar(&p.data))
                    span.pval { (fmt_pct(p.data.pct())) }
                }
            }
        }
    }
}

/// Change between the last two reports.
pub fn last_change(p: &Project) -> Markup {
    let Some(at) = p.last_report_at else {
        return html! { p.change.muted { "Waiting for the first progress report" } };
    };
    let dc = p.code.decompiled_pct() - p.prev_code.decompiled_pct();
    let dd = p.data.pct() - p.prev_data.pct();
    html! {
        p.change {
            span.delta.up[dc > 0.004].down[dc < -0.004] title="Code decompiled change in the last report" { "code " (fmt_delta(dc)) }
            @if p.data.total > 0 {
                " · "
                span.delta.up[dd > 0.004].down[dd < -0.004] title="Data change in the last report" { "data " (fmt_delta(dd)) }
            }
            " · " (time_ago(at))
        }
        @if let Some(msg) = &p.last_message {
            p.commit-msg title=(msg) { (msg) }
        }
    }
}

/// Up/down voting widget. Its state is filled in by `app.js` for logged-in users.
pub fn vote(p: &Project) -> Markup {
    html! {
        div.vote data-project=(p.id) data-slug=(p.slug) {
            button.up type="button" aria-label="Upvote" title="Upvote" disabled { "▲" }
            span.score { (p.score()) }
            button.down type="button" aria-label="Downvote" title="Downvote" disabled { "▼" }
        }
    }
}

pub fn card(p: &Project, extra: Option<Markup>) -> Markup {
    html! {
        article.card data-search=(search_text(p)) {
            a.card-cover href={ "/projects/" (p.slug) } tabindex="-1" { (cover(p, false)) }
            div.card-body {
                div.card-head {
                    (platform_chip(&p.platform))
                    h3 { a href={ "/projects/" (p.slug) } { (p.name) } }
                }
                (progress(p, false))
                (last_change(p))
                @if let Some(extra) = extra { (extra) }
            }
            (vote(p))
        }
    }
}

pub fn card_grid(projects: &[&Project]) -> Markup {
    html! {
        @if projects.is_empty() {
            p.muted { "No projects here yet." }
        } @else {
            div.grid { @for p in projects { (card(p, None)) } }
        }
    }
}

/// Legend for the area map, which only shows code.
pub fn legend_code() -> Markup {
    html! {
        ul.legend {
            li { span.sw.matching {} "Matching" }
            li { span.sw.nonmatching {} "Partially matching" }
            li { span.sw.none {} "Not decompiled" }
        }
    }
}

pub fn legend() -> Markup {
    html! {
        ul.legend {
            li { span.sw.matching {} "Matching" }
            li { span.sw.nonmatching {} "Partially matching" }
            li { span.sw.data {} "Data" }
            li { span.sw.none {} "Not decompiled" }
        }
    }
}

/// Lower-case text the search box matches against: name, platform and repository.
pub fn search_text(p: &Project) -> String {
    let plat = platforms::get_or_other(&p.platform);
    format!("{} {} {} {} {}", p.name, plat.short, plat.name, if p.repo_private { "" } else { &p.repo }, p.slug).to_lowercase()
}

/// GitHub mark (Octicons, MIT).
pub fn icon_github() -> Markup {
    PreEscaped(r#"<svg class="icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>"#.into())
}

/// Discord mark (Simple Icons, CC0).
pub fn icon_discord() -> Markup {
    PreEscaped(r#"<svg class="icon" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" fill="currentColor"><path d="M20.317 4.37a19.79 19.79 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.865-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.74 19.74 0 0 0 3.677 4.37a.07.07 0 0 0-.032.028C.533 9.046-.32 13.58.099 18.058a.082.082 0 0 0 .031.056 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028 14.09 14.09 0 0 0 1.226-1.994.076.076 0 0 0-.042-.106 13.11 13.11 0 0 1-1.872-.892.077.077 0 0 1-.008-.128c.126-.094.252-.192.372-.291a.074.074 0 0 1 .078-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.009c.12.099.246.198.373.292a.077.077 0 0 1-.006.127 12.3 12.3 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.84 19.84 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z"/></svg>"#.into())
}

/// RSS feed icon (Material Icons, Apache 2.0).
pub fn icon_rss() -> Markup {
    PreEscaped(r#"<svg class="icon" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" fill="currentColor"><circle cx="6.18" cy="17.82" r="2.18"/><path d="M4 4.44v2.83c7.03 0 12.73 5.7 12.73 12.73h2.83c0-8.59-6.97-15.56-15.56-15.56zm0 5.66v2.83c3.9 0 7.07 3.17 7.07 7.07h2.83c0-5.47-4.43-9.9-9.9-9.9z"/></svg>"#.into())
}

/// Generic repository icon for hosts other than GitHub (Octicons "repo", MIT).
pub fn icon_repo() -> Markup {
    PreEscaped(r#"<svg class="icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" fill="currentColor"><path d="M2 2.5A2.5 2.5 0 0 1 4.5 0h8.75a.75.75 0 0 1 .75.75v12.5a.75.75 0 0 1-.75.75h-2.5a.75.75 0 0 1 0-1.5h1.75v-2h-8a1 1 0 0 0-.714 1.7.75.75 0 1 1-1.072 1.05A2.495 2.495 0 0 1 2 11.5Zm10.5-1h-8a1 1 0 0 0-1 1v6.708A2.486 2.486 0 0 1 4.5 9h8ZM5 12.25a.25.25 0 0 1 .25-.25h3.5a.25.25 0 0 1 .25.25v3.25a.25.25 0 0 1-.4.2l-1.45-1.087a.249.249 0 0 0-.3 0L5.4 15.7a.25.25 0 0 1-.4-.2Z"/></svg>"#.into())
}

pub fn icon_lock() -> Markup {
    PreEscaped(r#"<svg class="icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" fill="currentColor"><path d="M4 4a4 4 0 0 1 8 0v2h.25c.966 0 1.75.784 1.75 1.75v5.5A1.75 1.75 0 0 1 12.25 15h-8.5A1.75 1.75 0 0 1 2 13.25v-5.5C2 6.784 2.784 6 3.75 6H4Zm8.25 3.5h-8.5a.25.25 0 0 0-.25.25v5.5c0 .138.112.25.25.25h8.5a.25.25 0 0 0 .25-.25v-5.5a.25.25 0 0 0-.25-.25ZM10.5 6V4a2.5 2.5 0 1 0-5 0v2Z"/></svg>"#.into())
}
