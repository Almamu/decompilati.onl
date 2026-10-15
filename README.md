# decompilati.onl

A progress tracker for video game decompilation projects. Developers log in with GitHub, register their
project, and report progress from CI through a webhook. The site shows code progress (matching /
non-matching / not decompiled) and data progress, a map of the project's areas, a history graph,
community votes, a highlights page, project search, and an AI badge for projects that say they're
mainly AI-generated.

It's a single Rust binary (axum + PostgreSQL) with server-rendered HTML and SVG. Nothing depends on an
external service except GitHub (login) and Cloudflare (CDN).

## Running locally

The app needs PostgreSQL (any recent version). It creates and migrates its tables itself.

```sh
createdb decompsite             # as a user PostgreSQL trusts, e.g. your own via peer auth
cp .env.example .env            # set DATABASE_URL if the default socket/user doesn't fit
cargo run -- seed-demo          # optional: fake projects (prints their API keys)
cargo run                       # http://localhost:8080
```

Databases from before the PostgreSQL switch can be copied over once, into an empty database:
`decompsite import-sqlite /path/to/decompsite.db`.

Login needs a GitHub OAuth app whose callback URL is `$BASE_URL/auth/github/callback`.

## How caching works

Public pages (`/`, `/projects`, `/platforms`, `/platform/*`, `/projects/*`, badges, JSON, RSS, docs) are the
same for every visitor. They never read or set cookies. Anything specific to the visitor (the user menu
and vote state) is filled in by `static/app.js` from `/api/me`. That call only happens when the
non-HttpOnly `li=1` hint cookie is present, so anonymous visitors never reach the origin.

1. **Origin memory cache.** Rendered pages are kept in memory with an ETag, so a Cloudflare miss
   doesn't touch the database. Conditional requests get a `304`.
2. **Cloudflare edge.** Responses carry `Cache-Control: public, max-age=60, s-maxage=2592000`.
   When a report, edit, vote or moderation action changes a project, the affected URLs are queued.
   After `PURGE_DEBOUNCE_SECS` the queue is purged in batches of 30 URLs using the Cloudflare API
   (purge-by-URL works on the free plan). Each URL is then re-requested through Cloudflare so the
   edge fetches the new version right away. Re-warming reaches the Cloudflare data center closest to
   the VPS; other data centers fetch on their next request, and those fetches are served from the
   memory cache.
3. **Assets** are compiled into the binary and served at content-hashed URLs with `immutable`.
   Cover images are re-encoded to 640×360 JPEG and stored under content-hash names, so they're
   immutable too.

If `CF_API_TOKEN` and `CF_ZONE_ID` aren't set, purging is disabled and the edge TTL drops to 60 s, so
nothing stays stale for long.

### Cloudflare setup (free plan)

- **SSL/TLS:** Full (strict), using a Cloudflare Origin Certificate on the VPS (see `deploy/Caddyfile`).
  Running `cloudflared` instead also works and avoids opening ports.
- **Caching → Cache Rules**, in this order:
  1. *Bypass:* when the URI path starts with `/dashboard`, `/admin`, `/auth` or `/api` → **Bypass cache**.
  2. *Cache HTML:* hostname equals `decompilati.onl` → **Eligible for cache**, Edge TTL **"Use
     cache-control header if present"**, Browser TTL **"Respect origin"**.
- **API token:** create one with the permission *Zone → Cache Purge → Purge* for this zone.
- Brotli and HTTP/3 are on by default. Leave Rocket Loader off; it isn't needed.

## Progress webhook

```
POST /api/v1/progress
Authorization: Bearer dk_…
```

The full format is documented on the site at `/docs` (source: `src/views/docs.rs`, `src/report.rs`).
In short: `code {total, matching, non_matching}`, `data {total, done}`, optional nested `areas` with
the same stats, plus `commit` and `message`. Reports that don't change anything are acknowledged and
then dropped.

The webhook also accepts an [objdiff](https://github.com/encounter/objdiff) `report.json`, the format
decomp.dev used. Send it as it is, optionally gzip-compressed with `Content-Encoding: gzip`, and pass
`?commit=<sha>`. Matched code counts as matching. Fuzzy-matched code beyond that counts as
non-matching (objdiff has no exact figure for it). Categories become areas. After each stored
report, the server fetches the commits since the previous report from GitHub (set
`GITHUB_API_TOKEN` to avoid the anonymous rate limit), and the project page lists them.

## Ranking and moderation

- A project's score is upvotes minus downvotes. Owners can't vote on their own projects.
- Projects with a negative score sink to the bottom of listings. At or below `HIDE_THRESHOLD`
  (default −5) they're hidden: listings put them behind a "low-rated" disclosure and the homepage
  leaves them out. Their own page still works.
- **Highlights** (`/highlights`, linked from the homepage): up to 12 projects, the ones admins
  (`ADMIN_LOGINS`) pin first, then the most upvoted.
- Admins can also hide projects and ban users from `/admin`. Banning a user signs them out and hides
  their projects.
- To register a repository, the user must have push access to it on GitHub. Only public
  repositories can be registered.

## Deploying on a VPS

```sh
cargo build --release
sudo install -m755 target/release/decompsite /usr/local/bin/
sudo useradd --system --home /var/lib/decompsite decompsite
sudo apt install postgresql                      # or any PostgreSQL server
sudo -u postgres createuser decompsite           # peer auth: the decompsite OS user logs in as it
sudo -u postgres createdb -O decompsite decompsite
sudo install -m600 .env /etc/decompsite.env      # edit first
sudo cp deploy/decompsite.service /etc/systemd/system/
sudo systemctl enable --now decompsite
```

Then put Caddy (`deploy/Caddyfile`) or nginx in front of it. A `Dockerfile` is also included.
The database is backed up daily by `deploy/backup.sh` (a compressed `pg_dump`, last 14 days kept in
`/var/backups/decompsite`), run by `deploy/decompsite-backup.timer`:
`sudo cp deploy/decompsite-backup.* /etc/systemd/system/ && sudo systemctl enable --now decompsite-backup.timer`.
Restore with `pg_restore --clean --if-exists -d decompsite <file>`. Also back up
and `DATA_DIR/uploads/`.

## Layout

```
src/main.rs          router, middleware, startup
src/config.rs        environment configuration
src/db.rs            schema/migrations and queries (PostgreSQL, r2d2 pool)
src/report.rs        webhook report format, validation and roll-up
src/cache.rs         in-memory page cache + Cloudflare purge/re-warm worker
src/auth.rs          GitHub OAuth, sessions, CSRF origin check, repo access check
src/images.rs        cover image processing
src/platforms.rs     supported platforms
src/routes/          public pages, API, dashboard/admin handlers
src/views/           HTML (maud) and SVG (treemap, history, badge) rendering
static/              CSS, JS, favicon (embedded into the binary)
```

## License

Copyright (C) 2026 Alexis Maiquez Murcia.

This program is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version. See [LICENSE](LICENSE) for the full text.
