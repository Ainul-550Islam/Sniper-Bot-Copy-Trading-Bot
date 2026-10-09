#!/usr/bin/env bash
# tests/release/buyer_parity.sh — buyer package parity + shape test
# (PROMPT 6 §J). This is the release-gate wrapper: it asserts the buyer
# package is not only in source parity with the canonical tree (the
# deeper drift-proving regression lives in buyer_source_parity.sh) but
# also SHIPPED COMPLETE — every directory the handover promises
# (source, docs, manifests, checksums, sbom, licenses, evidence) plus
# version coherence between the canonical tree and the buyer copy.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BUYER="$ROOT/buyer-release"
FAIL=0

echo "[buyer-parity] 1/4 — buyer source must be in byte parity with the canonical tree"
if ! bash "$ROOT/scripts/compare-canonical-to-buyer-source.sh" >/dev/null 2>&1; then
  echo "[buyer-parity] FAIL — source parity broken; run scripts/rebuild-buyer-release.sh" >&2
  FAIL=1
else
  echo "[buyer-parity] source parity: OK"
fi

echo "[buyer-parity] 2/4 — the promised package directories must exist and be non-empty"
for dir in source docs manifests checksums sbom licenses evidence; do
  if [ ! -d "$BUYER/$dir" ] || [ -z "$(ls -A "$BUYER/$dir" 2>/dev/null)" ]; then
    echo "[buyer-parity] FAIL — buyer-release/$dir missing or empty" >&2
    FAIL=1
  else
    echo "[buyer-parity] buyer-release/$dir: OK ($(ls -A "$BUYER/$dir" | wc -l) entries)"
  fi
done

echo "[buyer-parity] 3/4 — the handover papers must be present at the buyer root"
for paper in LICENSE VERSION CHANGELOG.md; do
  if [ ! -f "$BUYER/$paper" ]; then
    echo "[buyer-parity] FAIL — buyer-release/$paper missing" >&2
    FAIL=1
  else
    echo "[buyer-parity] buyer-release/$paper: OK"
  fi
done

echo "[buyer-parity] 4/4 — version coherence (canonical VERSION = buyer VERSION)"
CANON="$(cat "$ROOT/VERSION")"
BUYERV="$(cat "$BUYER/VERSION")"
if [ "$CANON" != "$BUYERV" ]; then
  echo "[buyer-parity] FAIL — version drift: canonical=$CANON buyer=$BUYERV" >&2
  FAIL=1
else
  echo "[buyer-parity] version coherence: OK ($CANON)"
fi

if [ "$FAIL" -ne 0 ]; then
  echo "[buyer-parity] FAIL — buyer package incomplete or out of parity" >&2
  exit 1
fi
echo "[buyer-parity] PASS — parity holds and the package is complete"
exit 0
