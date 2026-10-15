#!/bin/sh
# Pulls the latest code, rebuilds the release binary in a Rust container, restarts the
# service and purges Cloudflare (templates may have changed, so every cached page is stale).
# Run as root from the production checkout (/opt/decompilati.onl).
set -eu
cd "$(dirname "$0")/.."
git pull --ff-only
docker run --rm -v "$PWD":/src -v decompsite-cargo:/usr/local/cargo/registry -w /src \
    rust:1-bookworm cargo build --release
systemctl restart decompsite
systemctl is-active decompsite

CF_API_TOKEN=$(sed -n 's/^CF_API_TOKEN=//p' .env)
CF_ZONE_ID=$(sed -n 's/^CF_ZONE_ID=//p' .env)
if [ -n "$CF_API_TOKEN" ] && [ -n "$CF_ZONE_ID" ]; then
    curl -fsS -X POST "https://api.cloudflare.com/client/v4/zones/$CF_ZONE_ID/purge_cache" \
        -H "Authorization: Bearer $CF_API_TOKEN" -H "Content-Type: application/json" \
        --data '{"purge_everything":true}' >/dev/null && echo "cloudflare cache purged"
fi
