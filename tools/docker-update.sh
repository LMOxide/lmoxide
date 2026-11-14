#!/usr/bin/env bash
# Update a Docker deployment (docs/HOSTING.md "Updating"); safe to run from cron or a deploy hook.
#   tools/docker-update.sh            the published images (docker-compose.yml)
#   tools/docker-update.sh --build    lmoxide built from this checkout (docker-compose.build.yml)
# Pulls this repository (compose files, Caddyfile), then the images (or builds lmoxide), starts
# what changed, and recreates Caddy when docker/Caddyfile changed: a running container keeps the
# file it started with (git replaces the file; the bind mount still points at the old one).
# docker/lmoxide.toml and .env are yours: new settings a release needs are in its notes.
set -euo pipefail
cd "$(dirname "$0")/.."
BUILD=0
[[ ${1:-} == --build ]] && BUILD=1
before=$(git rev-parse HEAD)
git pull --ff-only --quiet
if (( BUILD )); then
  LMOXIDE_GIT_COMMIT=$(git rev-parse HEAD)
  LMOXIDE_GIT_COMMIT_TIME=$(git log -1 --format=%ct)
  export LMOXIDE_GIT_COMMIT LMOXIDE_GIT_COMMIT_TIME
  compose=(docker compose -f docker-compose.yml -f docker-compose.build.yml)
  "${compose[@]}" build lmoxide
else
  compose=(docker compose)
  "${compose[@]}" pull --quiet
fi
"${compose[@]}" up -d
if ! git diff --quiet "$before" HEAD -- docker/Caddyfile; then
  "${compose[@]}" up -d --force-recreate caddy
fi
