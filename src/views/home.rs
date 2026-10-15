use maud::{html, Markup};

use crate::{
    db::Project,
    platforms::{self, PLATFORMS},
    util::fmt_delta,
    views::components::{ai_badge, card, card_grid, cover, last_change, legend, platform_chip, progress, vote},
};

pub fn home(spotlight: &[&Project], movers: &[(&Project, f64)], recent: &[&Project], total: usize) -> Markup {
    html! {
        section.hero {
            h1 { "Tracking the progress of game decompilation projects" }
            p.lead {
                (total) " projects reverse-engineering games back into source code. "
                "Progress is updated automatically by each project's CI."
            }
            (legend())
        }

        @if !spotlight.is_empty() {
            section {
                h2.section-title { "Spotlight" }
                div.spotlight {
                    @for p in spotlight { (spotlight_card(p)) }
                }
            }
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
                h2.section-title { "Recently updated" }
                a href="/projects" { "All projects →" }
            }
            (card_grid(recent))
        }

        section {
            h2.section-title { "Browse by platform" }
            (platform_nav(None))
        }
    }
}

fn spotlight_card(p: &Project) -> Markup {
    html! {
        article.spot {
            a.card-cover href={ "/p/" (p.slug) } tabindex="-1" { (cover(p, true)) }
            div.card-body {
                div.card-head {
                    (platform_chip(&p.platform))
                    @if p.ai_generated { (ai_badge()) }
                    h3 { a href={ "/p/" (p.slug) } { (p.name) } }
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

pub fn platform_nav(active: Option<&str>) -> Markup {
    let mut groups: Vec<(&str, Vec<&platforms::Platform>)> = Vec::new();
    for p in PLATFORMS {
        match groups.last_mut() {
            Some((maker, list)) if *maker == p.maker => list.push(p),
            _ => groups.push((p.maker, vec![p])),
        }
    }
    html! {
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

pub fn listing(title: &str, intro: Option<&str>, projects: &[&Project], hidden: &[&Project], active_platform: Option<&str>) -> Markup {
    html! {
        section {
            h1 { (title) }
            @if let Some(i) = intro { p.lead { (i) } }
            (platform_nav(active_platform))
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
            @for (p, n) in counts {
                a.platform-row href={ "/platform/" (p.slug) } {
                    span.chip.big style={ "--chip:" (p.color) } { (p.short) }
                    span.pname { (p.name) span.muted { " · " (p.maker) } }
                    span.pcount { (n) " project" @if *n != 1 { "s" } }
                }
            }
        }
    }
}
