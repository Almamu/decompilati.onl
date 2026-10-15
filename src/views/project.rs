use maud::{html, Markup, PreEscaped};

use crate::github::{is_commit_hash, CommitList};

use crate::{
    db::{Project, ReportRow},
    platforms,
    report::{Area, Layout},
    util::{fmt_count, fmt_delta, fmt_pct},
    views::{
        components::{code_bar, cover, data_bar, icon_discord, icon_github, icon_rss, last_change, legend_code, platform_chip, vote},
        layout::time_ago,
        svg,
    },
};

pub fn project(p: &Project, reports: &[ReportRow], base_url: &str, hidden_by_votes: bool, versions: &[String], active: &str) -> Markup {
    let plat = platforms::get_or_other(&p.platform);
    let areas = p.areas();
    let with_functions = any_functions(&areas);
    html! {
        @if hidden_by_votes || p.hidden {
            div.notice { "This project is hidden from listings because of its community rating or moderation." }
        }
        section.project-head {
            div.project-cover { (cover(p, true)) }
            div.project-info {
                div.card-head {
                    (platform_chip(&p.platform))
                    span.muted { (plat.name) }
                }
                h1 { (p.name) }
                p.links {
                    "by " a href={ "https://github.com/" (p.owner_login) } rel="noopener" { "@" (p.owner_login) }
                }
                div.link-buttons {
                    a.btn.btn-github href={ "https://github.com/" (p.repo) } rel="noopener" title={ "github.com/" (p.repo) } {
                        (icon_github()) span { "GitHub" }
                    }
                    @if let Some(invite) = &p.discord {
                        a.btn.btn-discord href=(invite) rel="noopener nofollow" { (icon_discord()) span { "Discord" } }
                    }
                    a.btn.btn-rss href={ "/projects/" (p.slug) "/feed.xml" } title="Follow this project's progress in a feed reader" { (icon_rss()) span { "RSS" } }
                }
                @if let Some((value, label)) = crate::db::AI_CONTRIBUTIONS.iter().find(|(v, _)| !v.is_empty() && *v == p.ai_contributions) {
                    p.ai-policy { span class={ "policy policy-" (value) } title="Whether the project accepts contributions made with AI tools" { "AI contributions: " strong { (label) } } }
                }
                @if !p.description.is_empty() { p.desc.pre { (p.description) } }
                (vote(p))
            }
        }

        @if versions.len() > 1 {
            nav.versions aria-label="Game versions" {
                span.muted { "Version:" }
                @for v in versions {
                    @let href = if *v == p.default_version { format!("/projects/{}", p.slug) } else { format!("/projects/{}/versions/{}", p.slug, v) };
                    a.vtab.active[v == active] href=(href) {
                        @if v.is_empty() { "(unnamed)" } @else { (v) }
                        @if *v == p.default_version { span.muted { " ★" } }
                    }
                }
            }
        }

        section.panel {
            div.stat-cards {
                div.stat-card {
                    div.stat-head {
                        h2 { "Code" }
                        span.stat-big { (fmt_pct(p.code.decompiled_pct())) }
                    }
                    span.stat-sub { "decompiled of " (fmt_count(p.code.total)) " " (p.unit) }
                    (code_bar(&p.code))
                    ul.stat-parts {
                        li { span.sw.matching {} b { (fmt_pct(p.code.matching_pct())) } " matching" }
                        li { span.sw.nonmatching {} b { (fmt_pct(p.code.non_matching_pct())) } " partially matching" }
                        li { span.sw.none {} b { (fmt_pct(100.0 - p.code.decompiled_pct())) } " to do" }
                    }
                }
                @if p.data.total > 0 {
                    div.stat-card {
                        div.stat-head {
                            h2 { "Data" }
                            span.stat-big { (fmt_pct(p.data.pct())) }
                        }
                        span.stat-sub { "decompiled of " (fmt_count(p.data.total)) " " (p.unit) }
                        (data_bar(&p.data))
                        ul.stat-parts {
                            li { span.sw.data {} b { (fmt_count(p.data.done)) } " done" }
                            li { span.sw.none {} b { (fmt_count(p.data.total - p.data.done)) } " to do" }
                        }
                    }
                }
                @if p.functions.total > 0 {
                    div.stat-card {
                        div.stat-head {
                            h2 { "Functions" }
                            span.stat-big { (fmt_pct(p.functions.pct())) }
                        }
                        span.stat-sub { "matching of " (fmt_count(p.functions.total)) " functions" }
                        (functions_bar(&p.functions))
                        ul.stat-parts {
                            li { span.sw.matching {} b { (fmt_count(p.functions.matching)) } " matching" }
                            li { span.sw.none {} b { (fmt_count(p.functions.total - p.functions.matching)) } " to do" }
                        }
                    }
                }
            }
            (last_change(p))
            (recent_commits(p))
        }

        @if !areas.is_empty() {
            section.panel {
                h2 { "Areas" }
                (legend_code())
                @match p.layout {
                    Layout::Treemap => div.treemap-wrap { (PreEscaped(svg::treemap(&areas, &p.unit))) },
                    Layout::Grid => div.area-grid { @for a in &areas { (area_tile(a)) } },
                }
                details.area-table {
                    summary { "All areas as a table" }
                    table {
                        thead { tr { th { "Area" } th { "Matching" } th { "Partially matching" } th { "Data" } @if with_functions { th.num { "Functions" } } th.num { "Size (" (p.unit) ")" } } }
                        tbody { @for (i, a) in areas.iter().enumerate() { (area_rows(a, &format!("a{i}"), None, 0, with_functions)) } }
                    }
                }
            }
        }

        @if reports.len() >= 2 {
            section.panel {
                h2 { "History" }
                div.history-legend {
                    span { span.sw.decomp {} "Decompiled" }
                    span { span.sw.matching {} "Matching" }
                    @if p.data.total > 0 { span { span.sw.data {} "Data" } }
                }
                (PreEscaped(svg::history(reports)))
            }
        }

        @if !reports.is_empty() {
            section.panel {
                h2 { "Recent reports" }
                table.reports {
                    thead { tr { th { "When" } th { "Code" } th { "Matching" } th { "Data" } th { "Commit" } } }
                    tbody {
                        @for (i, r) in reports.iter().enumerate().rev().take(15) {
                            @let prev = if i > 0 { reports.get(i - 1) } else { None };
                            tr {
                                td { (time_ago(r.created_at)) }
                                td {
                                    (fmt_pct(r.code.decompiled_pct()))
                                    @if let Some(prev) = prev {
                                        " " span.delta.small { (fmt_delta(r.code.decompiled_pct() - prev.code.decompiled_pct())) }
                                    }
                                }
                                td { (fmt_pct(r.code.matching_pct())) }
                                td { @if r.data.total > 0 { (fmt_pct(r.data.pct())) } @else { "—" } }
                                td.commit {
                                    @if let Some(c) = &r.commit {
                                        @if is_commit_hash(c) {
                                            a href={ "https://github.com/" (p.repo) "/commit/" (c) } rel="noopener" { code { (&c[..7.min(c.len())]) } }
                                        } @else { code { (c) } }
                                    }
                                    @if let Some(m) = &r.message { " " span.muted { (m) } }
                                }
                            }
                        }
                    }
                }
            }
        }

        section.panel {
            h2 { "Badges" }
            p.muted { "For your README. They show the default version and update with every report." }
            @for (path, alt) in badge_list(p) {
                div.badge-row {
                    img src=(path) alt=(alt) height="20";
                    pre.code { "[![" (alt) "](" (base_url) (path) ")](" (base_url) "/projects/" (p.slug) ")" }
                }
            }
        }
    }
}

fn area_tile(a: &Area) -> Markup {
    html! {
        div.area-tile title=(a.description.as_deref().unwrap_or("")) {
            div.area-name { (a.name) span.pval { (fmt_pct(a.code.decompiled_pct())) } }
            (code_bar(&a.code))
            @if a.functions.total > 0 {
                div.muted.small { (fmt_count(a.functions.matching)) " / " (fmt_count(a.functions.total)) " functions matching" }
            }
            @if !a.children.is_empty() {
                ul.area-children {
                    @for c in &a.children {
                        li { span { (c.name) } span.muted { (fmt_pct(c.code.decompiled_pct())) } }
                    }
                }
            }
        }
    }
}

/// One table row per area. Children start collapsed; the toggle in their parent's row
/// (handled by app.js) shows them.
fn area_rows(a: &Area, id: &str, parent: Option<&str>, depth: usize, with_functions: bool) -> Markup {
    html! {
        tr data-row=(id) data-parent=[parent] hidden[parent.is_some()] {
            td style={ "padding-left:" (0.5 + depth as f64 * 1.2) "em" } {
                @if !a.children.is_empty() {
                    button.toggle type="button" data-toggle=(id) aria-expanded="false" title="Show what's inside" { "▸" }
                    " "
                }
                (a.name)
                @if !a.children.is_empty() { " " span.muted.small { "(" (a.children.len()) ")" } }
            }
            td { (fmt_pct(a.code.matching_pct())) }
            td { (fmt_pct(a.code.non_matching_pct())) }
            td { @if a.data.total > 0 { (fmt_pct(a.data.pct())) } @else { "—" } }
            @if with_functions {
                td.num { @if a.functions.total > 0 { (fmt_count(a.functions.matching)) " / " (fmt_count(a.functions.total)) } @else { "—" } }
            }
            td.num { (fmt_count(a.code.total)) }
        }
        @for (i, c) in a.children.iter().enumerate() {
            (area_rows(c, &format!("{id}-{i}"), Some(id), depth + 1, with_functions))
        }
    }
}

fn any_functions(areas: &[Area]) -> bool {
    areas.iter().any(|a| a.functions.total > 0 || any_functions(&a.children))
}

/// Commits fetched from GitHub after the last progress report.
fn recent_commits(p: &Project) -> Markup {
    let list: CommitList = serde_json::from_str(&p.commits_json).unwrap_or_default();
    if list.commits.is_empty() {
        return html! {};
    }
    html! {
        div.commits {
            h3 {
                "Latest commits"
                @if let Some(cmp) = &list.compare {
                    @if list.total > list.commits.len() {
                        " " a.muted href={ "https://github.com/" (p.repo) "/compare/" (cmp) } rel="noopener" { "(" (list.total) " in this update)" }
                    }
                }
            }
            ul {
                @for c in &list.commits {
                    li {
                        a href={ "https://github.com/" (p.repo) "/commit/" (c.sha) } rel="noopener" { code { (&c.sha[..7.min(c.sha.len())]) } }
                        " " span.cmsg { (c.message) }
                        span.muted {
                            @if let Some(a) = &c.author { " — " (a) }
                            @if let Some(ts) = c.date.as_deref().and_then(|d| time::OffsetDateTime::parse(d, &time::format_description::well_known::Rfc3339).ok()) {
                                " · " (time_ago(ts.unix_timestamp()))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Badge paths (relative to the site) and their alt text; data and functions only when reported.
fn badge_list(p: &Project) -> Vec<(String, &'static str)> {
    let mut list = vec![
        (format!("/badge/{}.svg", p.slug), "Decompiled"),
        (format!("/badge/{}/matching.svg", p.slug), "Matching"),
    ];
    if p.data.total > 0 {
        list.push((format!("/badge/{}/data.svg", p.slug), "Data"));
    }
    if p.functions.total > 0 {
        list.push((format!("/badge/{}/functions.svg", p.slug), "Functions"));
    }
    list
}

fn functions_bar(f: &crate::report::FunctionStats) -> Markup {
    let v = f.pct();
    html! {
        div.bar role="img" aria-label={ "Functions: " (fmt_pct(v)) " matching" } {
            span.seg.matching style={ "width:" (format!("{v:.3}")) "%" } {}
        }
    }
}
