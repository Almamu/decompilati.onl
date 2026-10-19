use std::collections::HashSet;

use maud::{html, Markup};

use crate::{
    db::Project,
    platforms::{self, PLATFORMS},
    views::components::{ai_badge, card_grid, cover, icon_rss, last_change, legend, platform_chip, progress, vote},
};

pub fn home(projects: &[&Project], total: usize) -> Markup {
    html! {
        section.hero {
            h1 { "Tracking the progress of game decompilation projects" }
            p.lead {
                (total) " projects reverse-engineering games back into source code."
            }
            div.home-tools {
                (search_box())
                a.btn.btn-ghost href="/highlights" { "★ Highlights" }
                a.btn.btn-rss href="/feed.xml" title="Follow every project's progress in a feed reader" { (icon_rss()) span { "RSS" } }
            }
        }

        section {
            details.about-details {
                summary { "What are decomps, and how does this site work?" }
                div.about {
                div {
                    h2 { "What's a decomp?" }
                    p { "A decompilation project rewrites a game's compiled machine code back into human-readable source code, usually C or C++, one function at a time. "
                        "Code is " strong { "matching" } " when it compiles to exactly the same bytes as the original game, and " strong { "partially matching" } " when it behaves the same but doesn't compile to identical bytes yet. "
                        "The result is real source code that people can study, port, fix and mod." }
                }
                div {
                    h2 { "Decomp vs. recomp" }
                    p { "A " strong { "recompilation" } " (static recomp) translates the original binary to another platform automatically, without recovering source code anyone can read. "
                        "It gets a game running natively quickly, but what it produces is machine-generated. "
                        "This site only tracks decompilations, where the code is reconstructed and understood. Projects that say they're mainly AI-generated get an " (ai_badge()) " badge." }
                }
                div {
                    h2 { "Community votes" }
                    p { "Logged-in users can upvote or downvote any project. "
                        "Votes decide what gets featured: low-rated projects sink in the listings and are hidden past a threshold, so stalled, misleading or spam entries don't crowd out real work." }
                }
                }
            }
            (legend())
        }
        @if !projects.is_empty() {
            section.browse-platforms {
                h2.section-title { "Browse by platform" }
                (platform_nav(None, projects))
            }
        }

        section {
            (card_grid(projects))
        }
    }
}

fn spotlight_card(p: &Project) -> Markup {
    html! {
        article.spot {
            a.card-cover href={ "/projects/" (p.slug) } tabindex="-1" { (cover(p, true)) }
            div.card-body {
                div.card-head {
                    (platform_chip(&p.platform))
                    h3 { a href={ "/projects/" (p.slug) } { (p.name) } }
                }
                @if !p.description.is_empty() {
                    p.desc { (truncate(&p.description, 180)) }
                }
                (progress(p, true))
                (last_change(p))
            }
            (vote(p))
        }
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

/// Chips for the platforms that have at least one listed project, grouped by maker.
pub fn platform_nav(active: Option<&str>, projects: &[&Project]) -> Markup {
    let present: HashSet<&str> = projects.iter().map(|p| p.platform.as_str()).collect();
    let mut groups: Vec<(&str, Vec<&platforms::Platform>)> = Vec::new();
    for p in PLATFORMS.iter().filter(|p| present.contains(p.slug) || active == Some(p.slug)) {
        match groups.last_mut() {
            Some((maker, list)) if *maker == p.maker => list.push(p),
            _ => groups.push((p.maker, vec![p])),
        }
    }
    html! {
        @if !groups.is_empty() {
            div.platforms {
                @for (maker, list) in &groups {
                    div.pgroup {
                        span.maker { (maker) }
                        @for p in list {
                            a.chip.big.active[active == Some(p.slug)] href={ "/platform/" (p.slug) } style={ "--chip:" (p.color) } title=(p.name) { (p.short) }
                        }
                    }
                }
            }
        }
    }
}

pub fn listing(title: &str, intro: Option<&str>, projects: &[&Project], hidden: &[&Project], all: &[&Project], active_platform: Option<&str>) -> Markup {
    html! {
        section {
            h1 { (title) }
            @if let Some(i) = intro { p.lead { (i) } }
            (search_box())
            (platform_nav(active_platform, all))
            (legend())
            (card_grid(projects))
        }
        @if !hidden.is_empty() {
            details.hidden-projects {
                summary { (hidden.len()) " low-rated project(s) hidden" }
                (card_grid(hidden))
            }
        }
    }
}

pub fn platforms_page(counts: &[(&'static platforms::Platform, usize)]) -> Markup {
    html! {
        h1 { "Platforms" }
        div.platform-table {
            @for (p, n) in counts.iter().filter(|(_, n)| *n > 0) {
                a.platform-row href={ "/platform/" (p.slug) } {
                    span.chip.big style={ "--chip:" (p.color) } { (p.short) }
                    span.pname { (p.name) span.muted { " · " (p.maker) } }
                    span.pcount { (n) " project" @if *n != 1 { "s" } }
                }
            }
        }
    }
}

/// Filters the project cards on the page as you type (handled by app.js).
pub fn search_box() -> Markup {
    html! {
        div.search hidden {
            input #project-search type="search" placeholder="Search projects by name, platform or repository…"
                aria-label="Search projects" autocomplete="off";
            p #search-empty.muted hidden { "No projects match your search." }
        }
    }
}

/// `/highlights`: projects pinned by admins first, then the most upvoted.
pub fn highlights(spotlight: &[&Project]) -> Markup {
    html! {
        section {
            h1 { "Highlights" }
            p.lead { "Projects picked by the site's admins, then the ones the community upvoted the most." }
            @if spotlight.is_empty() {
                p.muted { "Nothing here yet: projects show up once they get upvotes. " a href="/" { "Browse all projects" } "." }
            } @else {
                div.spotlight {
                    @for p in spotlight { (spotlight_card(p)) }
                }
            }
        }
    }
}

/// `/ports`: every listed project's ports, grouped by game.
pub fn ports_page(groups: &[(&Project, Vec<crate::db::Port>)]) -> Markup {
    let count: usize = groups.iter().map(|(_, ports)| ports.len()).sum();
    html! {
        section {
            h1 { "Ports" }
            p.lead { "Native versions and other ports built from the decompilations tracked here, as listed by each project. Most need assets from your own copy of the game." }
            @if groups.is_empty() {
                div.panel.empty { p.muted { "No project has listed a port yet." } }
            } @else {
                p.muted { (count) " port" @if count != 1 { "s" } " from " (groups.len()) " project" @if groups.len() != 1 { "s" } "." }
                @for (p, ports) in groups {
                    section.panel.port-group {
                        div.port-group-head {
                            (platform_chip(&p.platform))
                            h2 { a href={ "/projects/" (p.slug) } { (p.name) } }
                        }
                        (crate::views::project::port_list(ports))
                    }
                }
            }
        }
    }
}
