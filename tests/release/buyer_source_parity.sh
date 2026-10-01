#!/usr/bin/env bash
# tests/release/buyer_source_parity.sh — automated regression for
# buyer/source equality (PROMPT 5 §L, file 99).
#
# Two assertions:
#   1. the REAL buyer tree is in byte parity with the canonical product
#      (via scripts/compare-canonical-to-buyer-source.sh);
#   2. the compare itself actually detects drift — a scratch tree with a
#      planted tampered file MUST be flagged. A checker that always
#      says "OK" would be worthless, so this test proves the checker.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
COMPARE="$ROOT/scripts/compare-canonical-to-buyer-source.sh"

echo "[parity-test] 1/2 — the real buyer tree must be in parity"
if "$COMPARE"; then
  echo "[parity-test] real tree parity: OK"
else
  echo "[parity-test] real tree parity: DRIFT DETECTED (see report above)" >&2
  exit 1
fi

echo "[parity-test] 2/2 — the compare must catch a planted difference"
# Build a scratch release-shaped tree: scripts/compare… derives its ROOT
# from its own location, so placing a copy in $SCRATCH/scripts makes it
# compare $SCRATCH/buyer-release/source against $SCRATCH (which links
# back to the canonical top-level entries).
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

mkdir -p "$SCRATCH/scripts" "$SCRATCH/buyer-release/source"
cp "$COMPARE" "$SCRATCH/scripts/compare-canonical-to-buyer-source.sh"
cp -a "$ROOT/buyer-release/source/." "$SCRATCH/buyer-release/source/"
# Link the canonical top-level entries (everything the compare walks).
for entry in "$ROOT"/* "$ROOT"/.[!.]*; do
  name="$(basename "$entry")"
  [ "$name" = "buyer-release" ] && continue
  [ -e "$SCRATCH/$name" ] || ln -s "$entry" "$SCRATCH/$name"
done
# Plant the drift: tamper one known-parity file in the scratch buyer tree.
printf '\ntampered-by-parity-regression\n' >> "$SCRATCH/buyer-release/source/VERSION"

if "$SCRATCH/scripts/compare-canonical-to-buyer-source.sh" >/dev/null 2>&1; then
  echo "[parity-test] FAIL — the compare script did NOT flag the planted drift" >&2
  exit 1
else
  echo "[parity-test] planted drift detected — the compare fails closed as required"
fi

echo "[parity-test] PASS — buyer source parity holds and the checker is proven to detect drift"
exit 0
