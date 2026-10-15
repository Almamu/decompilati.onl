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
    pub slug: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub description: String,
    /// Checkbox: present ("on") when ticked.
    pub ai_generated: Option<String>,
}

impl ProjectForm {
    pub fn from_project(p: &Project) -> Self {
        ProjectForm {
            name: p.name.clone(),
            slug: p.slug.clone(),
            platform: p.platform.clone(),
            repo: p.repo.clone(),
            website: p.website.clone().unwrap_or_default(),
            description: p.description.clone(),
            ai_generated: p.ai_generated.then(|| "on".into()),
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
                            td { (platform_chip(&p.platform)) " " a href={ "/p/" (p.slug) } { (p.name) } }
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

fn form_fields(f: &ProjectForm) -> Markup {
    html! {
        label { "Game / project name"
            input type="text" name="name" value=(f.name) required maxlength="100" placeholder="Super Mario 64";
        }
        label { "URL slug " span.muted { "(optional, lowercase letters, digits and dashes)" }
            input type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*[a-z0-9]" placeholder="sm64";
        }
        label { "Platform"
            select name="platform" required {
                @for p in PLATFORMS {
                    option value=(p.slug) selected[f.platform == p.slug] { (p.name) }
                }
            }
        }
        label { "GitHub repository " span.muted { "(owner/name — you need push access)" }
            input type="text" name="repo" value=(f.repo) required maxlength="200" placeholder="n64decomp/sm64";
        }
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

pub fn new_project(f: &ProjectForm, error: Option<&str>) -> Markup {
    html! {
        section.narrow {
            p { a href="/dashboard" { "← Your projects" } }
            h1 { "Add a project" }
            @if let Some(e) = error { div.notice.err { (e) } }
            form.form method="post" action="/dashboard/new" {
                (form_fields(f))
                button.btn type="submit" { "Create project" }
            }
        }
    }
}

pub fn edit_project(u: &SessionUser, p: &Project, f: &ProjectForm, error: Option<&str>, notice: Option<&str>, new_key: Option<&str>, base_url: &str) -> Markup {
    html! {
        section.narrow {
            p { a href="/dashboard" { "← Your projects" } " · " a href={ "/p/" (p.slug) } { "View public page" } }
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
                form.form method="post" action={ "/dashboard/p/" (p.id) } {
                    (form_fields(f))
                    button.btn type="submit" { "Save changes" }
                }
            }

            div.panel {
                h2 { "Cover image" }
                p.muted { "PNG, JPEG, WebP or GIF, up to 5 MB. It is resized to 640×360 (16:9)." }
                @if p.image.is_some() { div.cover-preview { (crate::views::components::cover(p, true)) } }
                form.form method="post" action={ "/dashboard/p/" (p.id) "/image" } enctype="multipart/form-data" {
                    input type="file" name="image" accept="image/png,image/jpeg,image/webp,image/gif" required;
                    button.btn type="submit" { "Upload" }
                }
                @if p.image.is_some() {
                    form method="post" action={ "/dashboard/p/" (p.id) "/image/delete" } {
                        button.btn.btn-ghost type="submit" { "Remove image" }
                    }
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
                            td { (platform_chip(&p.platform)) " " a href={ "/p/" (p.slug) } { (p.name) } }
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
