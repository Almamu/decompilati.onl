#!/bin/sh
# Pulls the latest code, rebuilds the release binary in a Rust container and restarts the service.
# Run as root from the production checkout (/opt/decompilati.onl).
set -eu
cd "$(dirname "$0")/.."
git pull --ff-only
docker run --rm -v "$PWD":/src -v decompsite-cargo:/usr/local/cargo/registry -w /src \
    rust:1-bookworm cargo build --release
systemctl restart decompsite
systemctl is-active decompsite
