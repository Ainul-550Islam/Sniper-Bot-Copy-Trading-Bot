#!/usr/bin/env bash
# stats_current.sh — docs/STATS.md and every stat marker in buyer docs must
# match what the tree actually contains RIGHT NOW (P0-B TASK 3).
#
# Two checks:
#   1. REGENERATE — run scripts/generate-stats.sh into a temp file and diff
#      against the committed docs/STATS.md. Any drift fails the release.
#   2. MARKERS — scan README.md and top-level docs/*.md for
#      <!-- stat:KEY -->VALUE<!-- /stat --> markers. A marker fails when its
#      KEY is unknown to the generator or its VALUE differs from the
#      freshly computed one. No marker may carry a hand-typed number.
#
# Exits non-zero on any failure. Never edits anything: measure only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATS="$ROOT/docs/STATS.md"
fail=0

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

# --- check 1: committed STATS.md == freshly generated ----------------------
bash "$ROOT/scripts/generate-stats.sh" "$TMP" >/dev/null
if ! diff -u "$TMP" "$STATS" >/tmp/stats-drift.$$ 2>&1; then
    echo "[stats-current] FAIL — docs/STATS.md is stale:" >&2
    head -20 /tmp/stats-drift.$$ >&2
    rm -f /tmp/stats-drift.$$
    fail=1
else
    rm -f /tmp/stats-drift.$$
    echo "[stats-current] docs/STATS.md matches the tree"
fi

# --- check 2: markers in README + top-level docs ----------------------------
python3 - "$ROOT" "$TMP" <<'PY' || fail=1
import re, sys
from pathlib import Path

root = Path(sys.argv[1]); fresh = Path(sys.argv[2])
computed = dict(re.findall(
    r"<!-- stat:([a-z_]+) -->(.*?)<!-- /stat -->", fresh.read_text()))

MARK = re.compile(r"<!-- stat:([a-z_]+) -->(.*?)<!-- /stat -->")
scanned = [root / "README.md"] + sorted((root / "docs").glob("*.md"))
problems, seen = [], 0
for doc in scanned:
    if doc.name == "STATS.md":
        continue
    for m in MARK.finditer(doc.read_text(errors="replace")):
        seen += 1
        key, value = m.group(1), m.group(2)
        if key not in computed:
            problems.append(f"{doc.relative_to(root)}: unknown stat key '{key}'")
        elif str(computed[key]) != value:
            problems.append(
                f"{doc.relative_to(root)}: stat:{key} says '{value}' "
                f"but the tree computes '{computed[key]}'")

if problems:
    for p in problems:
        print(f"[stats-current] FAIL — {p}", file=sys.stderr)
    sys.exit(1)
print(f"[stats-current] {seen} stat markers checked, all current")
PY

if [ "$fail" -ne 0 ]; then
    echo "[stats-current] FAIL — stats are stale; run scripts/generate-stats.sh" >&2
    exit 1
fi
echo "[stats-current] PASS — STATS.md and all markers match the tree"
