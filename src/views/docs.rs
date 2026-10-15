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

            h2 { "objdiff reports (decomp.dev compatible)" }
            p { "If your project already generates an " a href="https://github.com/encounter/objdiff" { "objdiff" } " report for decomp.dev, send that file as it is. "
                "It's detected automatically (or force it with " code { "?format=objdiff" } "). Matched code counts as matching; fuzzy-matched code beyond that counts as non-matching; "
                "data uses objdiff's matched data; and " code { "progress_categories" } " become areas, with their units inside them." }
            p { "objdiff reports don't include a commit, so pass it in the query string. Reports can be large: send them gzip-compressed with "
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

            h2 { "Read-only endpoints" }
            ul {
                li { code { "GET /projects/{slug}/progress.json" } " — current progress of a project." }
                li { code { "GET /badge/{slug}.svg" } " — README badge." }
                li { code { "GET /feed.xml" } " — RSS feed of recent updates." }
            }
        }
    }
}
