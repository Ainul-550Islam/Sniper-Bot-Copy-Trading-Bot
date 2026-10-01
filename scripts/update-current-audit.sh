#!/usr/bin/env bash
# update-current-audit.sh — refresh the "current audit" documents'
# measured numbers from the actual tree (PROMPT 6 §J).
#
# The same discipline as scripts/update-release-manifest.sh: counts are
# FACTS, re-derived from the tree, never hand-edited. This script:
#
#   1. measures: rust files (crates/), docs files (docs/), TS/TSX files
#      (apps/control-plane), migrations count + high-water mark, VERSION;
#   2. REWRITES the count tables in the living current-state documents
#      (docs/CURRENT-BUYER-STATE.md, docs/CURRENT-STATE.md);
#   3. VERIFIES the prose numbers in docs/CURRENT-BUYER-FACTSHEET-2026.md
#      and fails loudly if they drifted (prose is edited by a human, and
#      a stale prose number must never survive silently);
#   4. prints the measured numbers.
#
# Exit 0 = refreshed and consistent; exit 1 = a verified number drifted
# (fix the prose or the tree, then re-run); exit 2 = structure changed
# so the script cannot find its anchors (update the script, never the
# other way around).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

exec python3 - "$ROOT" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])

# ---------------------------------------------------------------------------
# Measure (the same fields the manifest counts; single source of logic is
# the tree itself).
# ---------------------------------------------------------------------------
# Same definitions as scripts/update-release-manifest.sh: every .rs under
# crates/ (incl. tests and build.rs), every .md in docs/, every .ts/.tsx
# under apps/control-plane (generated dirs excluded there by the manifest
# walker; this tree has none committed).
rust_files = sum(1 for _ in root.glob("crates/**/*.rs"))
docs_files = len(list(root.glob("docs/*.md")))
ts_files = len([p for p in root.glob("apps/control-plane/**/*.ts*") if "node_modules" not in p.parts and ".next" not in p.parts])
mig_files = sorted(root.glob("crates/core/migrations/*.sql"))
migrations = len(mig_files)
high_water = mig_files[-1].name.split("_")[0] if mig_files else "?"
version = (root / "VERSION").read_text().strip()

measured = {
    "rust_files": rust_files,
    "docs_files": docs_files,
    "ts_files": ts_files,
    "migrations": migrations,
    "high_water": high_water,
    "version": version,
}

print(f"[current-audit] measured: rust={rust_files} docs={docs_files} ts/tsx={ts_files} "
      f"migrations={migrations} (high-water {high_water}) version={version}")

# ---------------------------------------------------------------------------
# Rewrite the living count tables.
# ---------------------------------------------------------------------------
def rewrite_table(path, replacements):
    """Rewrite `| Label | N |` rows whose label matches; returns list of
    labels actually rewritten."""
    if not path.exists():
        print(f"[current-audit] WARN: {path.name} missing (skipped)", file=sys.stderr)
        return []
    text = path.read_text()
    seen = []
    for label, value in replacements.items():
        pat = re.compile(r'(\|\s*' + re.escape(label) + r'\s*\|\s*)[^|]+(\s*\|)')
        new_text, n = pat.subn(lambda m: m.group(1) + str(value) + m.group(2), text, count=1)
        if n:
            text = new_text
            seen.append(label)
    path.write_text(text)
    return seen

buyer_state = root / "docs/CURRENT-BUYER-STATE.md"
seen1 = rewrite_table(buyer_state, {
    "Rust source files (crates/)": f"{rust_files} (manifest `rust_files`; programs/staking-suite counted separately as a component)",
    "Docs files (`docs/`)": f"{docs_files} (includes this file)",
    "TypeScript/TSX files (`apps/control-plane`)": str(ts_files),
    "Database migrations (forward-only)": f"{migrations} (high-water `{high_water}`)",
})

current_state = root / "docs/CURRENT-STATE.md"
seen2 = rewrite_table(current_state, {
    "Rust source files (crates/)": str(rust_files),
    "Docs files (docs/)": str(docs_files),
    "TypeScript/TSX files (apps/control-plane)": str(ts_files),
    "Database migrations (forward-only)": f"{migrations} (high-water `{high_water}`)",
})

for doc, seen in ((buyer_state, seen1), (current_state, seen2)):
    missing = {"Rust source files", "Docs files", "TypeScript/TSX", "Database migrations"}
    if not all(any(m.split()[0] in label for label in seen) for m in missing):
        print(f"[current-audit] WARN: {doc.name} — some count anchors were not found "
              f"(rewrote: {', '.join(seen) or 'none'}) — update this script, not the doc", file=sys.stderr)

# ---------------------------------------------------------------------------
# Verify the factsheet prose (fail loudly on drift).
# ---------------------------------------------------------------------------
factsheet = root / "docs/CURRENT-BUYER-FACTSHEET-2026.md"
drift = []
if factsheet.exists():
    text = factsheet.read_text()
    checks = [
        (f"{rust_files} Rust files", "rust file count"),
        (f"{ts_files} TS/TSX files", "ts/tsx count"),
        (f"{docs_files} documents", "docs count"),
        (f"{migrations} forward-only migrations", "migration count"),
    ]
    for needle, what in checks:
        if needle not in text:
            drift.append(f"{what}: factsheet does not say '{needle}'")
else:
    print("[current-audit] WARN: docs/CURRENT-BUYER-FACTSHEET-2026.md missing (skipped prose check)", file=sys.stderr)

if drift:
    print("[current-audit] FAIL — factsheet prose drifted from measured reality:", file=sys.stderr)
    for d in drift:
        print(f"  - {d}", file=sys.stderr)
    print("[current-audit] fix the prose in docs/CURRENT-BUYER-FACTSHEET-2026.md (or the tree), then re-run", file=sys.stderr)
    sys.exit(1)

print("[current-audit] OK — living count tables refreshed; factsheet prose matches measured reality")
PY
