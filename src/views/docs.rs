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

pub fn docs(base_url: &str) -> Markup {
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
            p.muted { "Limits: 1 MB per request, 120 reports per hour per project. Reports identical to the current state are accepted but not stored." }

            h2 { "GitHub Actions example" }
            pre.code { (GH_ACTION) (base_url) }

            h2 { "Read-only endpoints" }
            ul {
                li { code { "GET /p/{slug}/progress.json" } " — current progress of a project." }
                li { code { "GET /badge/{slug}.svg" } " — README badge." }
                li { code { "GET /feed.xml" } " — RSS feed of recent updates." }
            }
        }
    }
}
