use maud::{html, Markup};

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
        @if let Some(img) = &p.image {
            img.cover src={ "/uploads/" (img) } alt={ (p.name) " cover art" } width="640" height="360"
                loading=(if eager { "eager" } else { "lazy" }) decoding="async";
        } @else {
            div.cover.placeholder style={ "--chip:" (plat.color) } aria-hidden="true" {
                span { (plat.short) }
            }
        }
    }
}

/// Stacked bar for code: matching / non-matching / not decompiled.
pub fn code_bar(c: &CodeStats) -> Markup {
    let m = c.matching_pct();
    let n = c.non_matching_pct();
    let label = format!("Code: {} matching, {} non-matching", fmt_pct(m), fmt_pct(n));
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
        article.card {
            a.card-cover href={ "/p/" (p.slug) } tabindex="-1" { (cover(p, false)) }
            div.card-body {
                div.card-head {
                    (platform_chip(&p.platform))
                    @if p.ai_generated { (ai_badge()) }
                    h3 { a href={ "/p/" (p.slug) } { (p.name) } }
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

pub fn legend() -> Markup {
    html! {
        ul.legend {
            li { span.sw.matching {} "Matching" }
            li { span.sw.nonmatching {} "Non-matching" }
            li { span.sw.data {} "Data" }
            li { span.sw.none {} "Not decompiled" }
        }
    }
}
