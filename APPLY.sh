#!/usr/bin/env bash
# BATCH 01 apply helper.
#
# Unzip batch-01-rls-frontend-truth.zip over the repository root, then run this
# script. It does exactly two things, in order:
#
#   1. scripts/untrack-batch01-junk.sh — the repository's single tracked-junk
#      cleanup owner (delegates to scripts/cleanup-tracked-junk.sh --apply):
#      `git rm --cached` for the committed host state and local run scaffolding
#      (.cargo/bin/, .cargo/env, rustup-init.sh, core-launch.out,
#      run-tests.out, root run-*.sh / run_baseline*.sh launchers), removal of
#      the duplicate root AUDIT.md, untracking of the live config.toml, and
#      the matching .gitignore patterns. It is idempotent: a second run
#      reports "no tracked junk" and exits 0.
#   2. `git add -A` — stage everything so the integrator can review with
#      `git status` / `git diff --cached` and commit themselves.
#
# It never commits, never tags, never pushes, never force-pushes, and never
# rewrites history. Reviewing and committing the staged tree is the
# integrator's job.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"
bash scripts/untrack-batch01-junk.sh
git add -A
