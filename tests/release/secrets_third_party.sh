#!/usr/bin/env bash
# Third-party secret scan — gitleaks, run against the DELIVERED tree.
#
# scripts/scan-secrets.sh is this repository's own detector. This gate is the
# independent second opinion: Gitleaks 8.x (Apache-2.0, a tool written and
# maintained outside this project) with its default rule set, restricted to the
# files that actually ship (the tracked tree, never target/ or node_modules/).
#
# The first run of this gate, on 2026-11-01, reported 37 findings. Every one was
# inspected: all were test placeholders, public on-chain identifiers, or the
# custody secret-detector's own PEM marker literals. `.gitleaks.toml` now records
# exactly those cases as a reviewed allowlist — item by item, with the reason —
# so a genuinely new secret still fails. Re-run the baseline by passing
# --no-config through GITLEAKS_ARGS below.
#
# Degradation: without the gitleaks binary the section prints SKIPPED and the
# gate exits 0 (same pattern as helm_image_contract.sh), so CI without the tool
# is not a false failure.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

GITLEAKS="${GITLEAKS_BIN:-$(command -v gitleaks || true)}"
# convenience fallback for a local tool directory (${TOOLS_DIR:-$HOME/tools})
if [[ -z "$GITLEAKS" && -x "${TOOLS_DIR:-$HOME/tools}/gitleaks" ]]; then
  GITLEAKS="${TOOLS_DIR:-$HOME/tools}/gitleaks"
fi
if [[ -z "$GITLEAKS" ]]; then
  echo "SKIP  gitleaks not installed (set GITLEAKS_BIN to run this gate)"
  exit 0
fi

echo "== gitleaks ($("$GITLEAKS" version 2>&1 | head -1))"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# Stage exactly the files that ship: the git index when available (the ZIP has
# no .git, so fall back to a filtered walk).
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  while IFS= read -r f; do
    [[ -f "$f" ]] || continue
    mkdir -p "$STAGE/$(dirname "$f")"
    cp "$f" "$STAGE/$f"
  done < <(git ls-files)
else
  tar -cf - --exclude=./.git --exclude=./target --exclude=./node_modules \
    --exclude=./buyer-release --exclude=./apps/control-plane/node_modules \
    --exclude=./apps/control-plane/.next . 2>/dev/null | tar -xf - -C "$STAGE"
fi

if [[ -f .gitleaks.toml ]]; then
  cp .gitleaks.toml "$STAGE/"
fi

if out=$("$GITLEAKS" detect --source "$STAGE" --no-git --redact --config "$STAGE/.gitleaks.toml" \
  --report-format json --report-path "$STAGE/report.json" 2>&1); then
  echo "$out" | grep -E "no leaks found|leaks found" | tail -1
  echo "PASS  gitleaks: no secrets in the delivered tree"
  exit 0
fi

echo "FAIL  gitleaks reported findings:"
python3 - "$STAGE/report.json" <<'PY' 2>/dev/null || cat "$STAGE/report.json"
import json, sys
for f in json.load(open(sys.argv[1])):
    print(f"  {f['RuleID']}  {f['File']}:{f['StartLine']}")
PY
echo "  → triage each finding; either remove the secret or add a reviewed,"
echo "    reasoned entry to .gitleaks.toml (never a blanket path exclusion)."
exit 1
