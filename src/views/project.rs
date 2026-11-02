use maud::{html, Markup, PreEscaped};

use crate::github::{is_commit_hash, CommitList};

use crate::{
    db::{Port, Project, ReportRow},
    platforms,
    report::{Area, Layout},
    util::{fmt_count, fmt_delta, fmt_pct},
    views::{
        components::{code_bar, cover, data_bar, icon_discord, icon_download, icon_github, icon_lock, icon_repo, icon_rss, last_change, legend_code, platform_chip, vote},
        layout::time_ago,
        svg,
    },
};

pub fn project(
    p: &Project,
    reports: &[ReportRow],
    base_url: &str,
    hidden_by_votes: bool,
    versions: &[String],
    active: &str,
    ports: &[Port],
) -> Markup {
    let plat = platforms::get_or_other(&p.platform);
    let areas = p.areas();
    let with_functions = any_functions(&areas);
    let with_linked = any_linked(&areas);
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
                    @if p.repo_private {
                        span.btn.btn-private title="The source code is in a private repository" {
                            (icon_lock()) span { "Private repository" }
                        }
                    } @else if p.is_github() {
                        a.btn.btn-github href=(p.repo_link()) rel="noopener" title={ "github.com/" (p.repo) } {
                            (icon_github()) span { "GitHub" }
                        }
                    } @else {
                        a.btn.btn-repo href=(p.repo_link()) rel="noopener nofollow" title=(p.repo) {
                            (icon_repo()) span { (p.host_label()) }
                        }
                    }
                    @if let Some(invite) = &p.discord {
                        a.btn.btn-discord href=(invite) rel="noopener nofollow" { (icon_discord()) span { "Discord" } }
                    }
                    a.btn.btn-rss href={ "/projects/" (p.slug) "/feed.xml" } title="Follow this project's progress in a feed reader" { (icon_rss()) span { "RSS" } }
                    @if p.code.total > 0 {
                        @let page = if active == p.default_version { format!("/projects/{}", p.slug) } else { crate::util::version_page(&p.slug, active) };
                        a.btn.btn-ghost.btn-report href={ (page) "/report.json" } download={ (p.slug) @if active != p.default_version { "-" (active) } "-report.json" }
                            title="Download the progress as an objdiff report (objdiff-cli report JSON)" { (icon_download()) span { "objdiff report" } }
                        a.btn.btn-ghost.btn-report href={ (page) "/history.json" } download={ (p.slug) @if active != p.default_version { "-" (active) } "-history.json" }
                            title="Download the progress history in decomp.dev's format, for importing into other sites" { (icon_download()) span { "History" } }
                    }
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
                    @let href = if *v == p.default_version { format!("/projects/{}", p.slug) } else { crate::util::version_page(&p.slug, v) };
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
                        @if p.code.linked > 0 {
                            li title="Code in units built entirely from decompiled source, with nothing taken from the original binary" {
                                span.sw.linked {} b { (fmt_pct(p.code.linked_pct())) } " fully linked"
                            }
                        }
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

        @if !ports.is_empty() {
            (ports_section(ports))
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
                        thead { tr { th { "Area" } @if with_linked { th { "Fully linked" } } th { "Matching" } th { "Partially matching" } th { "Data" } @if with_functions { th.num { "Functions" } } th.num { "Size (" (p.unit) ")" } } }
                        tbody { @for (i, a) in areas.iter().enumerate() { (area_rows(a, &format!("a{i}"), None, 0, with_functions, with_linked)) } }
                    }
                }
            }
        }

        @if reports.len() >= 2 {
            section.panel {
                h2 { "History" }
                div.history-legend {
                    @if reports.iter().any(|r| r.code.linked > 0) { span { span.sw.linked {} "Fully linked" } }
                    span { span.sw.decomp {} "Decompiled" }
                    span { span.sw.matching {} "Matching" }
                    @if p.data.total > 0 { span { span.sw.data {} "Data" } }
                }
                (PreEscaped(svg::history(reports)))
                p.small { a href={ (version_base(p, active)) "/history" } { "Progress at every commit →" } }
            }
        }

        @if !reports.is_empty() {
            @let cols = ReportColumns::for_reports(reports);
            section.panel {
                div.panel-head {
                    h2 { "Recent reports" }
                    a href={ (version_base(p, active)) "/history" } { "All " (reports.len()) " report" @if reports.len() != 1 { "s" } " →" }
                }
                table.reports {
                    (cols.head())
                    tbody {
                        @for (i, r) in reports.iter().enumerate().rev().take(15) {
                            (report_row(p, r, i.checked_sub(1).map(|j| &reports[j]), cols))
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
fn area_rows(a: &Area, id: &str, parent: Option<&str>, depth: usize, with_functions: bool, with_linked: bool) -> Markup {
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
            @if with_linked { td { (fmt_pct(a.code.linked_pct())) } }
            td { (fmt_pct(a.code.matching_pct())) }
            td { (fmt_pct(a.code.non_matching_pct())) }
            td { @if a.data.total > 0 { (fmt_pct(a.data.pct())) } @else { "—" } }
            @if with_functions {
                td.num { @if a.functions.total > 0 { (fmt_count(a.functions.matching)) " / " (fmt_count(a.functions.total)) } @else { "—" } }
            }
            td.num { (fmt_count(a.code.total)) }
        }
        @for (i, c) in a.children.iter().enumerate() {
            (area_rows(c, &format!("{id}-{i}"), Some(id), depth + 1, with_functions, with_linked))
        }
    }
}

fn any_functions(areas: &[Area]) -> bool {
    areas.iter().any(|a| a.functions.total > 0 || any_functions(&a.children))
}

fn any_linked(areas: &[Area]) -> bool {
    areas.iter().any(|a| a.code.linked > 0 || any_linked(&a.children))
}

/// Commits fetched from GitHub after the last progress report.
fn recent_commits(p: &Project) -> Markup {
    let list: CommitList = serde_json::from_str(&p.commits_json).unwrap_or_default();
    // Fetched while the repository was public: keep its commits private now.
    if list.commits.is_empty() || p.repo_private {
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
    if p.code.linked > 0 {
        list.push((format!("/badge/{}/linked.svg", p.slug), "Fully linked"));
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

/// Ports built on the decompilation, as their owners describe them.
fn ports_section(ports: &[Port]) -> Markup {
    html! {
        section.panel #ports {
            h2 { "Ports" }
            p.muted { "Versions of the game built from this decompilation, listed by the project. Downloads may need your own copy of the game." }
            (port_list(ports))
        }
    }
}

/// Port cards, shared with the `/ports` page.
pub fn port_list(ports: &[Port]) -> Markup {
    html! {
        ul.ports {
            @for port in ports {
                li.port {
                    div.port-head {
                        a.port-name href=(port.url) rel="noopener nofollow ugc" { (port.name) }
                        @if !port.platforms.is_empty() { span.port-platforms { (port.platforms) } }
                    }
                    @if !port.description.is_empty() { p.port-desc.pre { (port.description) } }
                    span.muted.small { (link_host(&port.url)) }
                }
            }
        }
    }
}

/// The site a link points to, so readers see where it goes.
fn link_host(url: &str) -> &str {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

/// The project page of a version (`/projects/{slug}` for the default one), which the version's
/// other pages hang off.
fn version_base(p: &Project, version: &str) -> String {
    if version == p.default_version { format!("/projects/{}", p.slug) } else { crate::util::version_page(&p.slug, version) }
}

/// Which optional columns a list of reports needs.
#[derive(Clone, Copy)]
struct ReportColumns {
    linked: bool,
    data: bool,
}

impl ReportColumns {
    fn for_reports(reports: &[ReportRow]) -> Self {
        ReportColumns { linked: reports.iter().any(|r| r.code.linked > 0), data: reports.iter().any(|r| r.data.total > 0) }
    }
    fn head(self) -> Markup {
        html! {
            thead { tr {
                th { "When" }
                @if self.linked { th { "Fully linked" } }
                th { "Decompiled" }
                th { "Matching" }
                @if self.data { th { "Data" } }
                th { "Commit" }
            } }
        }
    }
}

/// A percentage with its change since the previous report.
fn pct_cell(v: f64, prev: Option<f64>) -> Markup {
    let d = prev.map(|p| v - p).filter(|d| d.abs() >= 0.005);
    html! {
        td.pct {
            (fmt_pct(v))
            @if let Some(d) = d { " " span.delta.small.up[d > 0.0].down[d < 0.0] { (fmt_delta(d)) } }
        }
    }
}

fn report_row(p: &Project, r: &ReportRow, prev: Option<&ReportRow>, cols: ReportColumns) -> Markup {
    html! {
        tr {
            td.when { (time_ago(r.created_at)) }
            @if cols.linked { (pct_cell(r.code.linked_pct(), prev.map(|q| q.code.linked_pct()))) }
            (pct_cell(r.code.decompiled_pct(), prev.map(|q| q.code.decompiled_pct())))
            (pct_cell(r.code.matching_pct(), prev.map(|q| q.code.matching_pct())))
            @if cols.data {
                @if r.data.total > 0 { (pct_cell(r.data.pct(), prev.filter(|q| q.data.total > 0).map(|q| q.data.pct()))) } @else { td { "—" } }
            }
            td.commit title=[r.message.as_deref()] {
                @if let Some(c) = &r.commit {
                    @if is_commit_hash(c) {
                        @if let Some(link) = p.commit_link(c) {
                            a href=(link) rel="noopener" { code { (&c[..7.min(c.len())]) } }
                        } @else { code { (&c[..7.min(c.len())]) } }
                    } @else { code { (c) } }
                }
                @if let Some(m) = &r.message { " " span.muted { (m.lines().next().unwrap_or("")) } }
            }
        }
    }
}

/// True if a report moved any of the figures shown in the history.
fn changed(r: &ReportRow, prev: &ReportRow) -> bool {
    r.code != prev.code || r.data != prev.data
}

pub const HISTORY_PAGE_SIZE: usize = 100;

/// `/projects/{slug}/history`: every report of a version, newest first, with what each one changed.
/// `reports` is oldest first.
pub fn history_page(p: &Project, reports: &[ReportRow], versions: &[String], active: &str, page: usize, only_changes: bool) -> Markup {
    let base = version_base(p, active);
    let cols = ReportColumns::for_reports(reports);
    let rows: Vec<(usize, &ReportRow)> = reports
        .iter()
        .enumerate()
        .filter(|(i, r)| !only_changes || *i == 0 || changed(r, &reports[i - 1]))
        .rev()
        .collect();
    let pages = rows.len().div_ceil(HISTORY_PAGE_SIZE).max(1);
    let page = page.clamp(1, pages);
    let shown = &rows[((page - 1) * HISTORY_PAGE_SIZE).min(rows.len())..(page * HISTORY_PAGE_SIZE).min(rows.len())];
    let link = |page: usize, changes: bool| {
        let mut q = Vec::new();
        if page > 1 { q.push(format!("page={page}")); }
        if changes { q.push("changes=1".to_string()); }
        if q.is_empty() { format!("{base}/history") } else { format!("{base}/history?{}", q.join("&")) }
    };
    html! {
        p.crumbs { a href=(base) { "← " (p.name) @if active != p.default_version { " (" (active) ")" } } }
        h1 { "History" }
        @if versions.len() > 1 {
            nav.versions aria-label="Game versions" {
                span.muted { "Version:" }
                @for v in versions {
                    a.vtab.active[v == active] href={ (version_base(p, v)) "/history" } {
                        @if v.is_empty() { "(unnamed)" } @else { (v) }
                        @if *v == p.default_version { span.muted { " ★" } }
                    }
                }
            }
        }
        @if let (Some(first), Some(last)) = (reports.first(), reports.last()) {
            section.panel {
                p {
                    (fmt_count(reports.len() as u64)) " report" @if reports.len() != 1 { "s" } " since " (time_ago(first.created_at)) "."
                    @if reports.len() > 1 {
                        " Since the first one: "
                        @if cols.linked { (fmt_delta(last.code.linked_pct() - first.code.linked_pct())) " fully linked, " }
                        (fmt_delta(last.code.decompiled_pct() - first.code.decompiled_pct())) " decompiled, "
                        (fmt_delta(last.code.matching_pct() - first.code.matching_pct())) " matching"
                        @if cols.data && first.data.total > 0 { ", " (fmt_delta(last.data.pct() - first.data.pct())) " data" }
                        "."
                    }
                }
                div.history-filter {
                    a.vtab.active[!only_changes] href=(link(1, false)) { "Every report" }
                    a.vtab.active[only_changes] href=(link(1, true)) { "Only reports that changed progress" }
                }
                table.reports.history-table {
                    (cols.head())
                    tbody {
                        @for (i, r) in shown {
                            (report_row(p, r, i.checked_sub(1).map(|j| &reports[j]), cols))
                        }
                    }
                }
                @if pages > 1 {
                    nav.pager aria-label="Pages" {
                        @if page > 1 { a.btn.btn-ghost href=(link(page - 1, only_changes)) rel="prev" { "← Newer" } }
                        span.muted { "Page " (page) " of " (pages) }
                        @if page < pages { a.btn.btn-ghost href=(link(page + 1, only_changes)) rel="next" { "Older →" } }
                    }
                }
            }
        } @else {
            div.panel.empty { p.muted { "No reports yet." } }
        }
    }
}
