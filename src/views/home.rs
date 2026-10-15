use std::collections::HashSet;

use maud::{html, Markup};

use crate::{
    db::Project,
    platforms::{self, PLATFORMS},
    util::fmt_delta,
    views::components::{ai_badge, card, card_grid, cover, last_change, legend, platform_chip, progress, vote},
};

pub fn home(spotlight: &[&Project], movers: &[(&Project, f64)], projects: &[&Project], total: usize) -> Markup {
    html! {
        section.hero {
            h1 { "Tracking the progress of game decompilation projects" }
            p.lead {
                (total) " projects reverse-engineering games back into source code."
            }
        }

        @if !spotlight.is_empty() {
            section {
                h2.section-title { "Highlights " span.muted { "· most upvoted by the community" } }
                div.spotlight {
                    @for p in spotlight { (spotlight_card(p)) }
                }
            }
        }

        section {
            div.about {
                div {
                    h2 { "What's a decomp?" }
                    p { "A decompilation project rewrites a game's compiled machine code back into human-readable source code, usually C or C++, one function at a time. "
                        "Code is " strong { "matching" } " when it compiles to exactly the same bytes as the original game, and " strong { "non-matching" } " when it behaves the same but doesn't compile to identical bytes yet. "
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
            (legend())
        }


        @if !movers.is_empty() {
            section {
                h2.section-title { "Most progress " span.muted { "· last 30 days" } }
                div.grid {
                    @for (p, gain) in movers {
                        (card(p, Some(html! { p.gain { "▲ " (fmt_delta(*gain)) " code in 30 days" } })))
                    }
                }
            }
        }

        section {
            div.section-head {
                h2.section-title { "All projects " span.muted { "· most recently updated first" } }
                a href="/platforms" { "By platform →" }
            }
            (card_grid(projects))
        }

        @if !projects.is_empty() {
            section {
                h2.section-title { "Browse by platform" }
                (platform_nav(None, projects))
            }
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
