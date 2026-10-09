#!/usr/bin/env bash
# tests/business/business-matrix-completeness.sh — the business matrix
# must be complete and machine-consistent (PROMPT 6 §P6).
#
# Three assertions:
#   1. docs/BUSINESS-MATRIX-2026.md contains all 7 spec-required rows
#      (Sniper, Copy Trading, Polymarket, Staking/Token/Fee, Telegram,
#      SaaS, BUSINESS / Commercial) and every required column is present
#      and non-empty for every row (11 spec columns);
#   2. every completeness percentage in the doc matches a fresh run of
#      scripts/generate-business-matrix.sh (the doc may not claim a
#      number the tree does not support);
#   3. no safe-claim cell contains a banned phrase outside a negation
#      (the unsafe-claim column is ALLOWED to name banned phrases —
#      that is its job).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
DOC="$ROOT/docs/BUSINESS-MATRIX-2026.md"
GEN="$ROOT/scripts/generate-business-matrix.sh"
FAIL=0

if [ ! -f "$DOC" ]; then
  echo "[biz-matrix] FAIL — $DOC missing" >&2
  exit 1
fi
if [ ! -x "$GEN" ]; then
  echo "[biz-matrix] FAIL — $GEN missing or not executable" >&2
  exit 2
fi

echo "[biz-matrix] 1/3 — all 7 spec business lines with all 11 spec columns"
LINES="Sniper|Copy Trading|Polymarket|Staking/Token/Fee|Telegram|SaaS|BUSINESS / Commercial"
for line in "Sniper" "Copy Trading" "Polymarket" "Staking/Token/Fee" "Telegram" "SaaS" "BUSINESS / Commercial"; do
  row="$(grep -F "| $line |" "$DOC" | head -1)"
  if [ -z "$row" ]; then
    echo "[biz-matrix] FAIL — no matrix row for '$line'" >&2
    FAIL=1
    continue
  fi
  # The full matrix table uses 11 data columns.
  cells="$(printf '%s' "$row" | awk -F'|' '{print NF-2}')"
  if [ "$cells" -lt 11 ]; then
    echo "[biz-matrix] FAIL — row '$line' has $cells columns (needs 11)" >&2
    FAIL=1
  else
    echo "[biz-matrix] row '$line': $cells columns OK"
  fi
  # no empty cell
  if printf '%s' "$row" | awk -F'|' '{for(i=2;i<NF;i++) if($i ~ /^[[:space:]]*$/) exit 1}'; then
    :
  else
    echo "[biz-matrix] FAIL — row '$line' has an empty cell" >&2
    FAIL=1
  fi
done

echo "[biz-matrix] 2/3 — completeness percentages must match a fresh measurement"
GENOUT="$("$GEN")"
printf '%s\n' "$GENOUT" > /tmp/biz-gen.out
DOC="$DOC" GENOUT_FILE=/tmp/biz-gen.out python3 - <<'PY'
import os
import re
import sys
from pathlib import Path

doc = Path(os.environ["DOC"]).read_text()
gen = Path(os.environ["GENOUT_FILE"]).read_text()
fail = False
for line in gen.splitlines():
    if not line.startswith("| ") or "Business line" in line or line.startswith("| ---"):
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if len(cells) < 5 or cells[0] in ("Business line",):
        continue
    name, pct_gen = cells[0], cells[3].rstrip("%")
    # The full matrix (11 spec columns) puts completeness % at data
    # column 3 (index 2); the measured snapshot table puts it at index 3.
    rows = [l for l in doc.splitlines() if l.startswith(f"| {name} |")]
    if not rows:
        print(f"[biz-matrix] FAIL — no doc row for '{name}'", file=sys.stderr)
        fail = True
        continue
    full_row = rows[0].strip().strip("|").split("|")
    pct_doc = full_row[2].strip().rstrip("%")
    if not re.match(r'^\d+$', pct_doc):
        # "100% (9/9 ...)" form — take the leading number
        m = re.match(r'^(\d+)', pct_doc)
        pct_doc = m.group(1) if m else pct_doc
    if pct_doc != pct_gen:
        print(f"[biz-matrix] FAIL — '{name}': doc says {pct_doc}%, fresh measurement says {pct_gen}% — regenerate the doc", file=sys.stderr)
        fail = True
    else:
        print(f"[biz-matrix] '{name}' completeness: {pct_doc}% OK")
sys.exit(1 if fail else 0)
PY
if [ $? -ne 0 ]; then FAIL=1; fi

if printf '%s\n' "$GENOUT" | grep -q "INVALID safe claim"; then
  echo "[biz-matrix] FAIL — generator rejected a safe claim (banned phrase)" >&2
  FAIL=1
fi

echo "[biz-matrix] 3/3 — safe-claim cells must not contain banned claims"
# The safe-claims column is data column 9 of the 11-column matrix.
# Banned phrases inside an explicit negation ("no live proof") are the
# honest form and pass; a bare banned claim fails.
DOC="$DOC" python3 - <<'PY'
import os
import re
import sys
from pathlib import Path

doc = Path(os.environ["DOC"]).read_text()
BANNED = ["guaranteed", "risk-free", "risk free", "profitable", "battle-tested",
          "zero-defect", "zero-loss", "fully audited", "fully isolated",
          "soc2", "mainnet-live"]
fail = False
for line in doc.splitlines():
    if not re.match(r'^\| (Sniper|Copy Trading|Polymarket|Staking/Token/Fee|Telegram|SaaS|BUSINESS / Commercial) \|', line):
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if len(cells) < 11:
        continue
    safe = cells[8]  # data column 9: Safe marketing claim
    for phrase in BANNED:
        for m in re.finditer(re.escape(phrase), safe, re.IGNORECASE):
            ctx = safe[max(0, m.start() - 14):m.end()]
            if "no " not in ctx and "not " not in ctx and "never" not in ctx:
                print(f"[biz-matrix] FAIL — safe-claim cell contains banned claim '{phrase}': …{ctx}", file=sys.stderr)
                fail = True
if not fail:
    print("[biz-matrix] safe-claim cells clean")
sys.exit(1 if fail else 0)
PY
if [ $? -ne 0 ]; then FAIL=1; fi

if [ "$FAIL" -ne 0 ]; then
  echo "[biz-matrix] FAIL — see above" >&2
  exit 1
fi
echo "[biz-matrix] PASS — 7 spec lines, 11 spec columns, machine-matched percentages, clean claims"
exit 0
