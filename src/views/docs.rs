use maud::{html, Markup};

const EXAMPLE: &str = r#"{
  "unit": "bytes",
  "code": { "total": 4194304, "matching": 1887436, "non_matching": 209715 },
  "data": { "total": 1048576, "done": 314572 },
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

const GH_ACTION: &str = r#"- name: Report progress
  if: github.ref == 'refs/heads/main'
  run: |
    python3 tools/progress.py --json > progress.json
    curl --fail -sS -X POST "$BASE/api/v1/progress" \
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

pub fn docs(base_url: &str, app_slug: Option<&str>) -> Markup {
    html! {
        article.docs.narrow {
            h1 { "Progress API" }
            p.lead { "Projects report their progress by POSTing a JSON document to the webhook. "
                "Every report updates the project page, the homepage and the history graph." }

            h2 { "1. Get an API key" }
            p { "Log in with GitHub, " a href="/dashboard/new" { "register your project" }
                " and copy the API key shown after creation. Store it as a secret in your CI (e.g. "
                code { "DECOMP_API_KEY" } "). You can regenerate it at any time from the dashboard." }

            h2 { "2. Send a report" }
            pre.code { "POST " (base_url) "/api/v1/progress\nAuthorization: Bearer dk_…\nContent-Type: application/json" }
            pre.code { (EXAMPLE) }
            p { "Responses are JSON: " code { "200 {\"status\":\"updated\"}" } ", "
                code { "200 {\"status\":\"unchanged\"}" } " when nothing changed, or " code { "4xx {\"error\":\"…\"}" } "." }

            h2 { "Fields" }
            table {
                thead { tr { th { "Field" } th { "Description" } } }
                tbody {
                    tr { td { code { "version" } } td { "Optional game version (e.g. " code { "us" } ", " code { "europe" } "). Each version keeps its own progress and history, and the project page shows a tab per version. Can also be given as " code { "?version=" } ". The project owner picks which version is shown on cards and listings." } }
                    tr { td { code { "unit" } } td { "Free text label for the numbers, e.g. " code { "bytes" } " or " code { "functions" } " (default " code { "bytes" } ")." } }
                    tr { td { code { "code.total" } } td { "Total amount of code." } }
                    tr { td { code { "code.matching" } } td { "Code decompiled and matching the original binary." } }
                    tr { td { code { "code.non_matching" } } td { "Code decompiled to equivalent but non-matching source." } }
                    tr { td { code { "data.total" } ", " code { "data.done" } } td { "Data (tables, assets, strings…) total and decompiled. Optional." } }
                    tr { td { code { "areas" } } td { "Optional list of areas / zones for the map. Each has a " code { "name" } ", optional "
                        code { "description" } ", " code { "code" } ", " code { "data" } ", " code { "weight" } " (tile size, defaults to code total) and nested "
                        code { "children" } " (up to 4 levels, 2000 areas). Missing totals are summed from children." } }
                    tr { td { code { "layout" } } td { code { "treemap" } " (default, tiles sized by weight) or " code { "grid" } " (equal-size cards)." } }
                    tr { td { code { "code" } " / " code { "data" } } td { "Top-level totals are optional when " code { "areas" } " are given: they're summed from the areas." } }
                    tr { td { code { "commit" } ", " code { "message" } } td { "Optional commit hash and message shown as the last change." } }
                }
            }
            p.muted { "Limits: 64 MB per request, 120 reports per hour per project. Reports identical to the current state are accepted but not stored." }

            h2 #objdiff { "objdiff reports (decomp.dev compatible)" }
            p { "If your project already generates an " a href="https://github.com/encounter/objdiff" { "objdiff" } " report for decomp.dev, send that file as it is. "
                "It's detected automatically (or force it with " code { "?format=objdiff" } "). Matched code counts as matching; fuzzy-matched code beyond that counts as non-matching; "
                "data uses objdiff's matched data; and " code { "progress_categories" } " become areas, with their units inside them." }
            p { "objdiff reports don't include a commit or a version, so pass them in the query string (" code { "?commit=…&version=us" } "). Reports can be large: send them gzip-compressed with "
                code { "Content-Encoding: gzip" } " (up to 64 MB per request)." }
            pre.code { (OBJDIFF_ACTION) (base_url) }
            p { "After each report, the project page lists the commits since the previous report, fetched from GitHub." }

            @if let Some(slug) = app_slug {
                h2 { "Automatic import with the GitHub App" }
                p { "Projects that already upload the objdiff report as a GitHub Actions artifact for decomp.dev can keep their workflow unchanged. "
                    "Register the project here, then " a href={ "https://github.com/apps/" (slug) "/installations/new" } rel="noopener" { "install the GitHub App" } " on the repository. "
                    "When a workflow run on the default branch finishes successfully, its " code { "<version>_report" } " artifact is downloaded and imported, with the run's commit. "
                    "If there are several versions, the first one alphabetically is used. Only JSON reports are supported, not binary protobuf ones." }
                pre.code { (ARTIFACT_STEP) }
            }

            h2 { "GitHub Actions example" }
            pre.code { (GH_ACTION) (base_url) }

            section #decomp-dev {
                h2 { "Coming from decomp.dev" }
                p { "decomp.dev read objdiff reports that CI uploaded as GitHub Actions artifacts. The same reports work here, and moving over takes a few minutes." }

                h3 { "1. Register the project" }
                p { a href="/dashboard/new" { "Add the project" } " while logged in with a GitHub account that can push to the repository. A cover image is required." }

                h3 { "2. Connect CI (pick one)" }
                p { strong { "With the GitHub App (no workflow changes)." } " If your workflow still uploads the report as an artifact named "
                    code { "<version>_report" } " (for example " code { "us_report" } "), "
                    @if let Some(slug) = app_slug {
                        a href={ "https://github.com/apps/" (slug) "/installations/new" } rel="noopener" { "install the GitHub App" }
                    } @else { "install the GitHub App" }
                    " on the repository. Each successful run on the default branch is imported automatically. If you removed the upload step when decomp.dev closed, add it back:" }
                pre.code { (ARTIFACT_STEP) }
                p { strong { "With the API." } " Send the report from CI with the project's API key; see "
                    a href="#objdiff" { "objdiff reports" } " for a ready-made step. Add " code { "version=…" } " to the URL if the project has several versions." }

                h3 { "3. Versions" }
                p { "Each " code { "<version>_report" } " artifact (or " code { "?version=" } ") becomes a version with its own progress, history and tab on the project page. "
                    "The project's settings choose which version is shown on cards, listings and the badge. Projects with a single unversioned report don't see any of this." }

                h3 { "4. Import your history" }
                p { strong { "From GitHub Actions:" } " on the project's dashboard page, \"Import history from GitHub Actions\" (needs the GitHub App) imports the report artifacts of past successful runs on the default branch, oldest first, each at the time of its run. "
                    "GitHub deletes artifacts after their retention period (90 days by default), so this usually covers the last few months." }
                p { strong { "Older history:" } " send reports with a " code { "timestamp" } " (Unix seconds or RFC 3339). Reports older than the current progress are only added to the history, and commits already in the history are skipped, so a backfill can be re-run safely. "
                    "For example, rebuilding the report for every 20th commit (adjust the build commands to your project):" }
                pre.code { (BACKFILL_HEAD) (base_url) (BACKFILL_SCRIPT) }
                p.muted { "Timestamped reports have their own limit of 3000 per hour per project." }

                h3 { "Differences from decomp.dev" }
                ul {
                    li { "objdiff has no exact \"decompiled but not matching\" figure, so non-matching code is estimated from the fuzzy match percentage." }
                    li { "Categories (" code { "progress_categories" } ") become the areas of the map, with their units inside." }
                    li { "Only JSON reports are supported, not binary protobuf ones." }
                    li { "Badges are at " code { (base_url) "/badge/<slug>.svg" } " and show the default version." }
                }
            }

            h2 { "Read-only endpoints" }
            ul {
                li { code { "GET /projects/{slug}/progress.json" } " — current progress of a project." }
                li { code { "GET /badge/{slug}.svg" } " — README badge." }
                li { code { "GET /feed.xml" } " — RSS feed of recent updates." }
            }
        }
    }
}
