//! Documentation pages: `/docs` and `/docs/{slug}`, sharing a sidebar.

use maud::{html, Markup};

pub struct DocPage {
    /// Path segment after `/docs/` (empty for the overview).
    pub slug: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
}

pub static PAGES: &[DocPage] = &[
    DocPage { slug: "", title: "Overview", summary: "What the site tracks and the ways to report progress." },
    DocPage { slug: "getting-started", title: "Getting started", summary: "Register a project, get an API key and send your first report." },
    DocPage { slug: "report-format", title: "Report format", summary: "Every field of a progress report, versions, functions and backfilling history." },
    DocPage { slug: "objdiff", title: "objdiff & GitHub App", summary: "Send objdiff reports, or let the GitHub App import them from your workflow runs." },
    DocPage { slug: "decomp-dev", title: "Coming from decomp.dev", summary: "Move a decomp.dev project over, including its history." },
    DocPage { slug: "badges", title: "Badges & data", summary: "README badges, the JSON progress endpoint and feeds." },
];

pub fn find(slug: &str) -> Option<&'static DocPage> {
    PAGES.iter().find(|p| p.slug == slug)
}

pub fn path(page: &DocPage) -> String {
    if page.slug.is_empty() { "/docs".into() } else { format!("/docs/{}", page.slug) }
}

const EXAMPLE: &str = r#"{
  "unit": "bytes",
  "code": { "total": 4194304, "matching": 1887436, "non_matching": 209715 },
  "data": { "total": 1048576, "done": 314572 },
  "functions": { "total": 2062, "matching": 1734 },
  "layout": "treemap",
  "commit": "9f2c1e0b7d",
  "message": "Match the remaining functions in the title screen",
  "areas": [
    {
      "name": "Engine",
      "children": [
        { "name": "Audio",  "code": { "total": 524288, "matching": 400000, "non_matching": 50000 } },
        { "name": "Render", "code": { "total": 786432, "matching": 200000, "non_matching": 90000 },
          "data": { "total": 65536, "done": 32768 } }
      ]
    },
    {
      "name": "Hyrule Field",
      "description": "Overworld actors and scene code",
      "code": { "total": 262144, "matching": 262144, "non_matching": 0 }
    }
  ]
}"#;

const MINIMAL: &str = r#"{ "code": { "total": 4194304, "matching": 1887436, "non_matching": 209715 } }"#;

const GH_ACTION: &str = r#"- name: Report progress
  if: github.ref == 'refs/heads/main'
  run: |
    python3 tools/progress.py --json > progress.json
    curl --fail -sS -X POST "$BASE/api/v1/progress?commit=$GITHUB_SHA" \
      -H "Authorization: Bearer ${{ secrets.DECOMP_API_KEY }}" \
      -H "Content-Type: application/json" \
      --data @progress.json
  env:
    BASE: "#;

const OBJDIFF_ACTION: &str = r#"- name: Generate progress report
  run: objdiff-cli report generate -o report.json

- name: Send progress to decompilati.onl
  if: github.ref == 'refs/heads/main'
  run: |
    gzip -kf report.json
    curl --fail -sS -X POST "$BASE/api/v1/progress?commit=$GITHUB_SHA" \
      -H "Authorization: Bearer ${{ secrets.DECOMP_API_KEY }}" \
      -H "Content-Encoding: gzip" \
      --data-binary @report.json.gz
  env:
    BASE: "#;

const ARTIFACT_STEP: &str = r#"- name: Upload progress report
  uses: actions/upload-artifact@v4
  with:
    name: us_report
    path: report.json"#;

const BACKFILL_HEAD: &str = "#!/bin/sh\n# Run from a clean checkout with DECOMP_API_KEY set; reports every 20th commit on main.\nBASE=";
const BACKFILL_SCRIPT: &str = r#"
for sha in $(git rev-list --first-parent --reverse main | awk 'NR % 20 == 0'); do
  git checkout -q "$sha" || continue
  make NON_MATCHING=1 report && objdiff-cli report generate -o report.json || continue
  ts=$(git show -s --format=%ct "$sha")
  gzip -kf report.json
  curl -sS -X POST "$BASE/api/v1/progress?commit=$sha&timestamp=$ts" \
    -H "Authorization: Bearer $DECOMP_API_KEY" \
    -H "Content-Encoding: gzip" --data-binary @report.json.gz
  echo
done
git checkout -q main"#;

/// Renders a documentation page with the sidebar and previous/next links.
pub fn page(current: &DocPage, base_url: &str, app_slug: Option<&str>) -> Markup {
    let index = PAGES.iter().position(|p| p.slug == current.slug).unwrap_or(0);
    let prev = index.checked_sub(1).and_then(|i| PAGES.get(i));
    let next = PAGES.get(index + 1);
    let body = match current.slug {
        "getting-started" => getting_started(base_url),
        "report-format" => report_format(base_url),
        "objdiff" => objdiff(base_url, app_slug),
        "decomp-dev" => decomp_dev(base_url, app_slug),
        "badges" => badges(base_url),
        _ => overview(),
    };
    html! {
        div.docs-layout {
            nav.docs-nav aria-label="Documentation" {
                span.maker { "Docs" }
                ul {
                    @for p in PAGES {
                        li { a.active[p.slug == current.slug] href=(path(p)) { (p.title) } }
                    }
                }
            }
            article.docs {
                h1 { (current.title) }
                (body)
                nav.docs-pager {
                    @if let Some(p) = prev { a.prev href=(path(p)) { span.muted { "← Previous" } br; (p.title) } } @else { span {} }
                    @if let Some(p) = next { a.next href=(path(p)) { span.muted { "Next →" } br; (p.title) } }
                }
            }
        }
    }
}

fn overview() -> Markup {
    html! {
        p.lead { "decompilati.onl tracks the progress of video game decompilation projects. Each project reports its own progress from CI; the site turns it into progress bars, a map of the game's areas, a history graph and badges." }
        h2 { "Ways to report progress" }
        div.docs-cards {
            a.docs-card href="/docs/getting-started" {
                strong { "The progress API" }
                span { "POST a small JSON report from any CI with the project's API key. Works for any toolchain." }
            }
            a.docs-card href="/docs/objdiff" {
                strong { "objdiff reports" }
                span { "Send objdiff's report.json as it is; matching, data, functions and categories are read from it." }
            }
            a.docs-card href="/docs/objdiff#github-app" {
                strong { "The GitHub App" }
                span { "No API key: install the app and keep uploading report artifacts, as decomp.dev required." }
            }
        }
        h2 { "All pages" }
        ul.docs-index {
            @for p in PAGES.iter().skip(1) {
                li { a href=(path(p)) { (p.title) } " — " span.muted { (p.summary) } }
            }
        }
    }
}

fn getting_started(base_url: &str) -> Markup {
    html! {
        h2 { "1. Register your project" }
        p { "Log in with GitHub and " a href="/dashboard/new" { "add your project" } ". For code on GitHub, pick the repository from the public repositories you can push to; code hosted elsewhere (GitLab, Codeberg, self-hosted…) is added by its URL. A cover image is required." }
        p.muted { "Projects outside GitHub report progress through the API only: the GitHub App, history imports from GitHub Actions or decomp.dev, and commit lists need a GitHub repository." }

        h2 { "2. Keep the API key" }
        p { "The API key is shown once, right after creating the project. Store it as a secret in your CI, for example " code { "DECOMP_API_KEY" } ". You can generate a new one at any time from the project's dashboard page; the old one stops working immediately." }

        h2 { "3. Send a report" }
        p { "The smallest report only has code totals:" }
        pre.code { "curl -X POST " (base_url) "/api/v1/progress \\\n  -H \"Authorization: Bearer $DECOMP_API_KEY\" \\\n  -H \"Content-Type: application/json\" \\\n  --data '" (MINIMAL) "'" }
        p { "Responses are JSON:" }
        ul {
            li { code { "{\"status\":\"updated\"}" } " — the report was stored." }
            li { code { "{\"status\":\"unchanged\"}" } " — identical to the current progress, so nothing was stored." }
            li { code { "{\"status\":\"history\"}" } " — an older, timestamped report added to the history (see " a href="/docs/report-format#history" { "backfilling" } ")." }
            li { code { "4xx {\"error\":\"…\"}" } " — something is wrong with the key or the report; the message says what." }
        }
        p { "See " a href="/docs/report-format" { "Report format" } " for everything a report can contain, or " a href="/docs/objdiff" { "objdiff & GitHub App" } " if your project uses objdiff." }

        h2 { "4. Automate it in CI" }
        p { "Report from your main branch only, so pull requests don't change the public numbers. A GitHub Actions step:" }
        pre.code { (GH_ACTION) (base_url) }
        p { "After each report, the project page also lists the commits since the previous one, fetched from GitHub." }
    }
}

fn report_format(base_url: &str) -> Markup {
    html! {
        p.lead { "Reports are POSTed as JSON to " code { (base_url) "/api/v1/progress" } " with " code { "Authorization: Bearer <API key>" } "." }

        h2 { "Full example" }
        pre.code { (EXAMPLE) }

        h2 { "Fields" }
        table {
            thead { tr { th { "Field" } th { "Description" } } }
            tbody {
                tr { td { code { "code.total" } } td { "Total amount of code." } }
                tr { td { code { "code.matching" } } td { "Code decompiled and matching the original binary." } }
                tr { td { code { "code.non_matching" } } td { "Code decompiled to equivalent but only partially matching source." } }
                tr { td { code { "data.total" } ", " code { "data.done" } } td { "Data (tables, assets, strings…) total and decompiled. Optional." } }
                tr { td { code { "functions.total" } ", " code { "functions.matching" } } td { "Function counts, shown on the project page and the functions badge. Optional." } }
                tr { td { code { "unit" } } td { "Label for the numbers, e.g. " code { "bytes" } " or " code { "functions" } " (default " code { "bytes" } ")." } }
                tr { td { code { "areas" } } td { "Optional areas for the map; see below." } }
                tr { td { code { "layout" } } td { code { "treemap" } " (default, tiles sized by weight) or " code { "grid" } " (equal-size cards)." } }
                tr { td { code { "commit" } ", " code { "message" } } td { "Optional commit hash and message, shown as the last change." } }
                tr { td { code { "version" } } td { "Optional game version; see below." } }
            }
        }

        h2 #areas { "Areas" }
        p { "Areas (or zones) split the game into parts, drawn as a map where each tile fills up with its progress. Each area has a " code { "name" } " and optionally a "
            code { "description" } ", " code { "code" } ", " code { "data" } ", a " code { "weight" } " (tile size, defaults to the code total) and nested "
            code { "children" } ", up to 4 levels and 2000 areas in total." }
        p { "Totals that are missing are summed from the children, and the top-level " code { "code" } " and " code { "data" } " are summed from the areas when left out." }

        h2 #versions { "Versions" }
        p { "Projects that decompile several releases of a game (for example " code { "us" } " and " code { "europe" } ") can report each one separately with a " code { "version" } " field, or "
            code { "?version=" } " in the URL. Each version keeps its own progress and history, and the project page gets a tab per version. "
            "The project's settings choose which version is shown on cards, listings and badges." }
        p { "Versions are opt-in: reports without a version all go to one unnamed version, and nothing about versions is shown until a project reports two. "
            "When a project that reported without a version starts reporting several, \"Merge or rename versions\" on its dashboard page moves the unnamed history into one of the new versions; the same form renames a version." }

        h2 #history { "Backfilling history" }
        p { "Add " code { "?timestamp=" } " (Unix seconds or RFC 3339) to send a report from the past. Reports older than the current progress are only added to the history graph; the current progress doesn't change. "
            "A commit that's already in the history is skipped, so a backfill can be re-run safely. "
            a href="/docs/decomp-dev#history" { "Coming from decomp.dev" } " has an example script." }

        h2 { "Query parameters" }
        table {
            thead { tr { th { "Parameter" } th { "Description" } } }
            tbody {
                tr { td { code { "version" } } td { "Game version, for reports that don't include one." } }
                tr { td { code { "commit" } ", " code { "message" } } td { "Commit hash and message, for reports that don't include them (objdiff)." } }
                tr { td { code { "timestamp" } } td { "When the report was produced, to backfill history." } }
                tr { td { code { "format" } } td { code { "objdiff" } " to force reading an objdiff report; it's normally detected." } }
            }
        }

        h2 { "Limits" }
        ul {
            li { "64 MB per request (after decompression; send " code { "Content-Encoding: gzip" } " to compress)." }
            li { "120 reports per hour per project, and 3000 per hour for timestamped (backfill) reports." }
            li { "Reports identical to the current progress are accepted but not stored." }
        }
    }
}

fn objdiff(base_url: &str, app_slug: Option<&str>) -> Markup {
    html! {
        p.lead { "Projects using " a href="https://github.com/encounter/objdiff" { "objdiff" } " can send its " code { "report.json" } " as it is, the same file decomp.dev used." }

        h2 { "Sending objdiff reports" }
        p { "POST the report to the progress API like any other report. It's detected automatically (or force it with " code { "?format=objdiff" } "). "
            "objdiff reports don't include a commit or a version, so pass them in the URL: " code { "?commit=…&version=us" } ". "
            "With every function in it the report can be several megabytes, so send it gzip-compressed:" }
        pre.code { (OBJDIFF_ACTION) (base_url) }

        h2 { "How the numbers are read" }
        table {
            thead { tr { th { "objdiff" } th { "decompilati.onl" } } }
            tbody {
                tr { td { "matched code" } td { "matching code" } }
                tr { td { "fuzzy match beyond matched code" } td { "partially matching code (an estimate: objdiff has no exact figure for it)" } }
                tr { td { "matched data" } td { "data" } }
                tr { td { "matched / total functions" } td { "functions" } }
                tr { td { code { "progress_categories" } } td { "areas, with their units inside" } }
            }
        }
        p.muted { "Only JSON reports are supported, not binary protobuf ones." }

        h2 #github-app { "The GitHub App" }
        p { "Instead of sending reports, you can let the site fetch them: "
            @if let Some(slug) = app_slug {
                a href={ "https://github.com/apps/" (slug) "/installations/new" } rel="noopener" { "install the GitHub App" }
            } @else { "install the GitHub App" }
            " on the repository and upload the report as a GitHub Actions artifact named " code { "<version>_report" } ". No API key is needed." }
        pre.code { (ARTIFACT_STEP) }
        p { "When a workflow run on the default branch finishes successfully, every " code { "<version>_report" } " artifact in it is imported as that version (an artifact named just "
            code { "report" } " goes to the unnamed version), with the run's commit." }

        h2 #history-import { "Importing past runs" }
        p { "With the app installed, the project's dashboard page has an \"Import history from GitHub Actions\" button. It imports the report artifacts of past successful runs on the default branch, oldest first, each at the time of its run. "
            "GitHub deletes artifacts after their retention period (90 days by default); for older history, " a href="/docs/report-format#history" { "backfill through the API" } "." }
    }
}

fn decomp_dev(base_url: &str, app_slug: Option<&str>) -> Markup {
    html! {
        p.lead { "decomp.dev read objdiff reports that CI uploaded as GitHub Actions artifacts. The same reports work here, and moving over takes a few minutes." }

        h2 { "1. Register the project" }
        p { a href="/dashboard/new" { "Add the project" } " while logged in with a GitHub account that can push to the repository. A cover image is required." }
        p { "If the repository is on decomp.dev, its whole history (every version, with the latest report's areas) is imported automatically right after the project is created. There's nothing else to do for step 4." }

        h2 { "2. Connect CI (pick one)" }
        h3 { "With the GitHub App: no workflow changes" }
        p { "If your workflow still uploads the report as an artifact named " code { "<version>_report" } " (for example " code { "us_report" } "), "
            @if let Some(slug) = app_slug {
                a href={ "https://github.com/apps/" (slug) "/installations/new" } rel="noopener" { "install the GitHub App" }
            } @else { "install the GitHub App" }
            " on the repository. Each successful run on the default branch is imported automatically. If you removed the upload step when decomp.dev closed, add it back:" }
        pre.code { (ARTIFACT_STEP) }
        h3 { "With the API" }
        p { "Send the report from CI with the project's API key; " a href="/docs/objdiff" { "objdiff & GitHub App" } " has a ready-made step. Add " code { "version=…" } " to the URL if the project has several versions." }

        h2 { "3. Versions" }
        p { "Each " code { "<version>_report" } " artifact (or " code { "?version=" } ") becomes a version with its own progress, history and tab on the project page. "
            "The project's settings choose which version is shown on cards, listings and badges." }

        h2 #history { "4. Import your history" }
        p { strong { "From decomp.dev: " } "this happens automatically when the project is added. To run it again (it's safe: reports already in the history are skipped), \"Import from decomp.dev\" on the project's dashboard page takes the project's decomp.dev URL (for example " code { "https://decomp.dev/owner/repo" } ") and imports its whole history: every version and every report, with the latest report's areas. It doesn't need the GitHub App, and it can be run again safely." }
        p { strong { "From GitHub Actions: " } "on the project's dashboard page, \"Import history from GitHub Actions\" (needs the GitHub App) imports the report artifacts of past successful runs, each at the time of its run. "
            "GitHub deletes artifacts after their retention period (90 days by default), so this usually covers the last few months." }
        p { strong { "Anything else: " } "rebuild the report for past commits and send it with a " code { "timestamp" } ". For example, every 20th commit (adjust the build commands to your project):" }
        pre.code { (BACKFILL_HEAD) (base_url) (BACKFILL_SCRIPT) }
        p.muted { "Re-running it is safe: commits already in the history are skipped." }

        h2 #links { "5. Old links keep working" }
        p { "decomp.dev URLs work on this domain too: replace " code { "decomp.dev" } " with " code { "decompilati.onl" } " and " code { "/{owner}/{repo}" } ", " code { "/{owner}/{repo}/{version}" } ", " code { ".json" } " and " code { ".svg" } " badge links (with " code { "measure=code" } ", " code { "data" } " or " code { "functions" } ") redirect to the project registered for that repository." }

        h2 { "Differences from decomp.dev" }
        ul {
            li { "objdiff has no exact \"partially matching\" figure, so partially matching code is estimated from the fuzzy match percentage." }
            li { "Categories (" code { "progress_categories" } ") become the areas of the map, with their units inside." }
            li { "Only JSON reports are supported, not binary protobuf ones." }
            li { "Badges have their own URLs (see " a href="/docs/badges" { "Badges & data" } "), but old decomp.dev badge links redirect to them." }
            li { "The map of areas shows code only; data appears in the progress cards and the area table." }
        }
    }
}

fn badges(base_url: &str) -> Markup {
    html! {
        h2 { "Badges" }
        p { "Each project page lists its badges with ready-to-paste Markdown. They show the default version and update with every report." }
        table {
            thead { tr { th { "URL" } th { "Shows" } } }
            tbody {
                tr { td { code { "/badge/{slug}.svg" } } td { "Decompiled code (matching + partially matching)." } }
                tr { td { code { "/badge/{slug}/matching.svg" } } td { "Matching code." } }
                tr { td { code { "/badge/{slug}/data.svg" } } td { "Data, or \"n/a\" if the project doesn't report it." } }
                tr { td { code { "/badge/{slug}/functions.svg" } } td { "Matched / total functions, or \"n/a\"." } }
            }
        }
        pre.code { "[![Decompiled](" (base_url) "/badge/{slug}.svg)](" (base_url) "/projects/{slug})" }

        h2 { "Progress JSON" }
        p { code { "GET /projects/{slug}/progress.json" } " returns a project's current progress: code, data and functions, the areas, the last report and every version." }

        h2 { "Feeds" }
        ul {
            li { code { "GET /feed.xml" } " — RSS feed of every project's progress updates." }
            li { code { "GET /projects/{slug}/feed.xml" } " — RSS feed of one project's updates, every version." }
            li { code { "GET /sitemap.xml" } " — every public page, for search engines." }
        }
    }
}
