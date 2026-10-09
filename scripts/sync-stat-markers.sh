#!/usr/bin/env bash
# sync-stat-markers.sh — copy generated stat values into buyer/doc markers.
#
# docs/STATS.md is written by scripts/generate-stats.sh and is the ONLY source of
# repository numbers. Other docs embed the same numbers as
#   <!-- stat:KEY -->VALUE<!-- /stat -->
# This script rewrites VALUE from docs/STATS.md for every marker whose KEY exists
# there. It never invents a number, never edits text outside a marker, and is
# idempotent: a second run changes nothing.
#
# Order of operations:  scripts/generate-stats.sh  →  scripts/sync-stat-markers.sh
#                       →  tests/release/stats_current.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

python3 - "$ROOT" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
stats_path = root / "docs" / "STATS.md"
if not stats_path.is_file():
    sys.exit("sync-stat-markers: docs/STATS.md missing; run scripts/generate-stats.sh first")

marker = re.compile(r"<!-- stat:([A-Za-z0-9_]+) -->(.*?)<!-- /stat -->", re.S)
values = {}
for key, value in marker.findall(stats_path.read_text(encoding="utf-8")):
    if key in values and values[key] != value:
        sys.exit(f"sync-stat-markers: conflicting generated values for {key}")
    values[key] = value
if not values:
    sys.exit("sync-stat-markers: no stat markers found in docs/STATS.md")

changed = 0
unknown = set()
for path in sorted((root / "docs").rglob("*.md")):
    text = path.read_text(encoding="utf-8")
    if "<!-- stat:" not in text:
        continue

    def replace(match):
        global changed
        key, current = match.group(1), match.group(2)
        if key not in values:
            unknown.add((str(path.relative_to(root)), key))
            return match.group(0)
        if current == values[key]:
            return match.group(0)
        changed += 1
        return f"<!-- stat:{key} -->{values[key]}<!-- /stat -->"

    new_text = marker.sub(replace, text)
    if new_text != text:
        path.write_text(new_text, encoding="utf-8")

for path, key in sorted(unknown):
    print(f"sync-stat-markers: WARN unknown key {key} in {path} (left unchanged)")
print(f"sync-stat-markers: {changed} marker value(s) updated from docs/STATS.md")
PY
