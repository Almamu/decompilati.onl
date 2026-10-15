use maud::{html, Markup};
use serde::Deserialize;

use crate::{
    auth::SessionUser,
    db::{Project, User},
    platforms::PLATFORMS,
    util::fmt_pct,
    views::{components::platform_chip, layout::time_ago},
};

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ProjectForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub description: String,
    /// Version shown on cards and listings (edit form only, when several have reported).
    #[serde(default)]
    pub default_version: Option<String>,
    /// Checkbox: present ("on") when ticked.
    pub ai_generated: Option<String>,
}

impl ProjectForm {
    pub fn from_project(p: &Project) -> Self {
        ProjectForm {
            name: p.name.clone(),
            platform: p.platform.clone(),
            repo: p.repo.clone(),
            website: p.website.clone().unwrap_or_default(),
            description: p.description.clone(),
            ai_generated: p.ai_generated.then(|| "on".into()),
            default_version: Some(p.default_version.clone()),
        }
    }
}

pub fn dashboard(u: &SessionUser, projects: &[Project]) -> Markup {
    html! {
        div.section-head {
            h1 { "Your projects" }
            div {
                a.btn href="/dashboard/new" { "+ Add project" }
                @if u.is_admin { " " a.btn.btn-ghost href="/admin" { "Admin" } }
            }
        }
        p.muted { "Logged in as @" (u.user.login) "." }
        @if projects.is_empty() {
            div.panel.empty {
                p { "You haven't registered any project yet." }
                p { "Register a decompilation project hosted on GitHub that you can push to, then report progress from your CI using the "
                    a href="/docs" { "webhook API" } "." }
            }
        } @else {
            table.dash {
                thead { tr { th { "Project" } th { "Code" } th { "Data" } th { "Last report" } th {} } }
                tbody {
                    @for p in projects {
                        tr {
                            td { (platform_chip(&p.platform)) " " a href={ "/projects/" (p.slug) } { (p.name) } @if p.image.is_none() { " " span.badge.warn title="Upload a cover image to list this project" { "needs image" } } }
                            td { (fmt_pct(p.code.decompiled_pct())) }
                            td { @if p.data.total > 0 { (fmt_pct(p.data.pct())) } @else { "—" } }
                            td { @if let Some(t) = p.last_report_at { (time_ago(t)) } @else { span.muted { "never" } } }
                            td { a.btn.btn-small href={ "/dashboard/p/" (p.id) } { "Manage" } }
                        }
                    }
                }
            }
        }
        form method="post" action="/auth/logout" { button.btn.btn-ghost type="submit" { "Log out" } }
    }
}

/// Repositories offered in the project form's dropdown.
pub enum RepoOptions {
    List(Vec<String>),
    /// The GitHub token expired; the user has to log in again to refresh it.
    Expired { next: String },
    Failed,
}

fn repo_field(f: &ProjectForm, repos: &RepoOptions) -> Markup {
    html! {
        label { "GitHub repository " span.muted { "(public repositories you can push to)" }
            @match repos {
                RepoOptions::List(list) => {
                    select name="repo" required {
                        option value="" disabled selected[f.repo.is_empty()] { "Select a repository…" }
                        // Keep the current repository selectable even if it's no longer in the list.
                        @if !f.repo.is_empty() && !list.iter().any(|r| r.eq_ignore_ascii_case(&f.repo)) {
                            option value=(f.repo) selected { (f.repo) }
                        }
                        @for r in list {
                            option value=(r) selected[r.eq_ignore_ascii_case(&f.repo)] { (r) }
                        }
                    }
                    @if list.is_empty() {
                        span.muted.hint { "No public repositories with push access were found. Repositories in organizations only show up if the organization allows this app (" a href="https://github.com/settings/connections/applications" rel="noopener" { "GitHub settings" } ")." }
                    }
                }
                RepoOptions::Expired { next } => {
                    span.notice.err { "Your GitHub session has expired. " a href={ "/auth/github?next=" (crate::auth::urlencode(next)) } { "Log in again" } " to load your repositories." }
                }
                RepoOptions::Failed => {
                    span.notice.err { "Couldn't load your repositories from GitHub. Please reload the page to try again." }
                }
            }
        }
    }
}

fn form_fields(f: &ProjectForm, repos: &RepoOptions) -> Markup {
    html! {
        label { "Game / project name"
            input type="text" name="name" value=(f.name) required maxlength="100" placeholder="Super Mario 64";
        }
        label { "Platform"
            select name="platform" required {
                @for p in PLATFORMS {
                    option value=(p.slug) selected[f.platform == p.slug] { (p.name) }
                }
            }
        }
        (repo_field(f, repos))
        label { "Website " span.muted { "(optional)" }
            input type="url" name="website" value=(f.website) maxlength="300" placeholder="https://";
        }
        label { "Description"
            textarea name="description" rows="4" maxlength="2000" { (f.description) }
        }
        label.check {
            input type="checkbox" name="ai_generated" checked[f.ai_generated.is_some()];
            span { "This project is mainly AI-generated " span.muted { "(shows an AI badge)" } }
        }
    }
}

pub fn new_project(f: &ProjectForm, repos: &RepoOptions, error: Option<&str>) -> Markup {
    html! {
        section.narrow {
            p { a href="/dashboard" { "← Your projects" } }
            h1 { "Add a project" }
            @if let Some(e) = error { div.notice.err { (e) } }
            form.form method="post" action="/dashboard/new" enctype="multipart/form-data" {
                (form_fields(f, repos))
                label { "Cover image " span.muted { "(required — PNG, JPEG, WebP or GIF up to 5 MB, cropped to 16:9)" }
                    input type="file" name="image" accept="image/png,image/jpeg,image/webp,image/gif" required;
                }
                p.muted { "The project's URL is generated from its name and stays the same if you rename it later." }
                button.btn type="submit" disabled[!matches!(repos, RepoOptions::List(_))] { "Create project" }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn edit_project(
    u: &SessionUser,
    p: &Project,
    f: &ProjectForm,
    repos: &RepoOptions,
    error: Option<&str>,
    notice: Option<&str>,
    new_key: Option<&str>,
    base_url: &str,
    app_slug: Option<&str>,
    versions: &[String],
) -> Markup {
    html! {
        section.narrow {
            p { a href="/dashboard" { "← Your projects" } " · " a href={ "/projects/" (p.slug) } { "View public page" } }
            h1 { (p.name) }
            @if let Some(e) = error { div.notice.err { (e) } }
            @if let Some(n) = notice { div.notice.ok { (n) } }

            @if let Some(key) = new_key {
                div.panel.key {
                    h2 { "Your API key" }
                    p { "Copy it now — it won't be shown again." }
                    pre.code.select { (key) }
                }
            }

            div.panel {
                h2 { "Details" }
                p.muted { "Public URL: " a href={ "/projects/" (p.slug) } { (base_url) "/projects/" (p.slug) } }
                form.form method="post" action={ "/dashboard/p/" (p.id) } {
                    (form_fields(f, repos))
                    @if versions.len() > 1 {
                        label { "Default version " span.muted { "(shown on the homepage, listings and the badge)" }
                            select name="default_version" {
                                @for v in versions {
                                    option value=(v) selected[f.default_version.as_deref() == Some(v.as_str())] {
                                        @if v.is_empty() { "(unnamed)" } @else { (v) }
                                    }
                                }
                            }
                        }
                    }
                    button.btn type="submit" { "Save changes" }
                }
            }

            div.panel {
                h2 { "Cover image" }
                p.muted { "Required. PNG, JPEG, WebP or GIF, up to 5 MB. It is resized to 640×360 (16:9)." }
                @if p.image.is_none() { div.notice.err { "This project has no cover image, so it isn't shown in any listing. Upload one to publish it." } }
                @if p.image.is_some() { div.cover-preview { (crate::views::components::cover(p, true)) } }
                form.form method="post" action={ "/dashboard/p/" (p.id) "/image" } enctype="multipart/form-data" {
                    input type="file" name="image" accept="image/png,image/jpeg,image/webp,image/gif" required;
                    button.btn type="submit" { @if p.image.is_some() { "Replace image" } @else { "Upload" } }
                }
            }

            div.panel {
                h2 { "Progress webhook" }
                p { "Send progress reports from your CI with the API key (starts with " code { (p.api_key_prefix) "…" } "). See the " a href="/docs" { "API documentation" } " for the full report format." }
                pre.code { "curl -X POST " (base_url) "/api/v1/progress \\\n  -H \"Authorization: Bearer $DECOMP_API_KEY\" \\\n  -H \"Content-Type: application/json\" \\\n  --data @progress.json" }
                form method="post" action={ "/dashboard/p/" (p.id) "/key" } {
                    button.btn.btn-ghost type="submit" { "Regenerate API key" }
                    span.muted { " The current key stops working immediately." }
                }
            }

            @if let Some(slug) = app_slug {
                div.panel {
                    h2 { "Automatic import (decomp.dev compatible)" }
                    p { "If your CI uploads objdiff's " code { "report.json" } " as an artifact named " code { "<version>_report" } " (as decomp.dev required), install the GitHub App on "
                        code { (p.repo) } ". Every successful workflow run on the default branch is then imported automatically, with no API key needed." }
                    p { a.btn href={ "https://github.com/apps/" (slug) "/installations/new" } rel="noopener" { "Install the GitHub App" } }
                }
            }

            @if u.is_admin {
                div.panel {
                    h2 { "Moderation" }
                    form.inline method="post" action={ "/admin/p/" (p.id) "/spotlight" } {
                        button.btn.btn-ghost type="submit" { @if p.spotlight { "Remove from spotlight" } @else { "Add to spotlight" } }
                    }
                    form.inline method="post" action={ "/admin/p/" (p.id) "/hide" } {
                        button.btn.btn-ghost type="submit" { @if p.hidden { "Unhide" } @else { "Hide from listings" } }
                    }
                }
            }

            div.panel.danger {
                h2 { "Delete project" }
                p { "This permanently deletes the project and its history." }
                form.form method="post" action={ "/dashboard/p/" (p.id) "/delete" } {
                    label { "Type the slug " code { (p.slug) } " to confirm"
                        input type="text" name="confirm" required autocomplete="off";
                    }
                    button.btn.btn-danger type="submit" { "Delete project" }
                }
            }
        }
    }
}

pub fn admin(projects: &[Project], users: &[User]) -> Markup {
    html! {
        h1 { "Admin" }
        div.panel {
            h2 { "Projects" }
            table.dash {
                thead { tr { th { "Project" } th { "Owner" } th { "Score" } th { "Flags" } th {} } }
                tbody {
                    @for p in projects {
                        tr {
                            td { (platform_chip(&p.platform)) " " a href={ "/projects/" (p.slug) } { (p.name) } }
                            td { "@" (p.owner_login) }
                            td { (p.score()) " (" (p.upvotes) "/" (p.downvotes) ")" }
                            td {
                                @if p.spotlight { span.badge { "spotlight" } " " }
                                @if p.hidden { span.badge.warn { "hidden" } " " }
                                @if p.ai_generated { span.badge.ai { "AI" } }
                            }
                            td.actions {
                                form.inline method="post" action={ "/admin/p/" (p.id) "/spotlight" } {
                                    button.btn.btn-small.btn-ghost type="submit" { @if p.spotlight { "Unspotlight" } @else { "Spotlight" } }
                                }
                                form.inline method="post" action={ "/admin/p/" (p.id) "/hide" } {
                                    button.btn.btn-small.btn-ghost type="submit" { @if p.hidden { "Unhide" } @else { "Hide" } }
                                }
                                a.btn.btn-small.btn-ghost href={ "/dashboard/p/" (p.id) } { "Edit" }
                            }
                        }
                    }
                }
            }
        }
        div.panel {
            h2 { "Users" }
            table.dash {
                thead { tr { th { "User" } th { "Status" } th {} } }
                tbody {
                    @for u in users {
                        tr {
                            td { a href={ "https://github.com/" (u.login) } rel="noopener" { "@" (u.login) } @if let Some(n) = &u.name { " " span.muted { (n) } } }
                            td { @if u.is_banned { span.badge.warn { "banned" } } @else { "active" } }
                            td {
                                form.inline method="post" action={ "/admin/u/" (u.id) "/ban" } {
                                    button.btn.btn-small.btn-ghost type="submit" { @if u.is_banned { "Unban" } @else { "Ban" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
