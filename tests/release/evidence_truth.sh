#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$ROOT" <<'PY'
import json
import pathlib
import re
import sys
from collections import Counter

root = pathlib.Path(sys.argv[1])
evidence = root / "evidence"
files = sorted(evidence.rglob("*.json"))
counts: Counter[str] = Counter()

def collect(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "status" and isinstance(item, str):
                counts[item.upper()] += 1
            collect(item)
    elif isinstance(value, list):
        for item in value:
            collect(item)

for path in files:
    try:
        collect(json.loads(path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"FAIL: cannot read evidence JSON {path.relative_to(root)}: {error}")

actual_passed = counts["PASSED"]
print(f"Evidence status values across {len(files)} JSON files: " + ", ".join(
    f"{status}={count}" for status, count in sorted(counts.items())
))

# Validate explicit evidence-count claims in Markdown, not historical test
# totals or ordinary prose about passing a command.
claim = re.compile(r"\b(\d+)\s+(?:of\s+\d+\s+)?PASSED\b")
markdown = list(root.glob("*.md")) + list((root / "docs").rglob("*.md"))
for path in markdown:
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not re.search(r"evidence|external validation|live validation", line, re.IGNORECASE):
            continue
        for match in claim.finditer(line):
            claimed = int(match.group(1))
            if claimed > actual_passed:
                rel = path.relative_to(root)
                raise SystemExit(
                    f"FAIL: {rel}:{line_number} claims {claimed} PASSED evidence items; "
                    f"evidence JSON contains {actual_passed}"
                )
print("evidence truth gate PASS")
PY
