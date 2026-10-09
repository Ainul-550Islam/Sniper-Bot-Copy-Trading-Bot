#!/usr/bin/env bash
# tests/release/marketing_claims.sh — regression gate for the
# marketing-claim rules (PROMPT 6 §J).
#
# Two assertions:
#   1. the real tree passes scripts/verify-marketing-claims.sh (no
#      claim exceeds its evidence);
#   2. the checker actually REJECTS a planted unsupported claim — a
#      gate that cannot fail is worthless, so this test plants a
#      banned phrase in a scratch marketing-facing file inside a
#      scratch copy of the tree and requires the checker to fail.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
GATE="$ROOT/scripts/verify-marketing-claims.sh"

if [ ! -x "$GATE" ]; then
  echo "[claims-test] missing executable $GATE" >&2
  exit 2
fi

echo "[claims-test] 1/2 — the real tree must pass the claim gate"
if ! "$GATE"; then
  echo "[claims-test] FAIL — unsupported claims present (see report above)" >&2
  exit 1
fi
echo "[claims-test] real tree: OK"

echo "[claims-test] 2/2 — the gate must reject a planted unsupported claim"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# The gate derives its ROOT from its own location: copy it into a
# scratch tree, link the real docs/, and plant a banned phrase.
mkdir -p "$SCRATCH/scripts" "$SCRATCH/docs"
cp "$GATE" "$SCRATCH/scripts/verify-marketing-claims.sh"
for d in "$ROOT"/*; do
  name="$(basename "$d")"
  [ "$name" = "scripts" ] && continue
  [ "$name" = "README.md" ] && continue
  [ -e "$SCRATCH/$name" ] || ln -s "$d" "$SCRATCH/$name"
done
for f in "$ROOT"/scripts/*; do
  name="$(basename "$f")"
  [ "$name" = "verify-marketing-claims.sh" ] && continue
  ln -sf "$(readlink -f "$f")" "$SCRATCH/scripts/$name"
done
# README.md is a scanned marketing-facing file: plant the banned phrases
# there (as a real file copy, so the canonical README is never touched).
cp "$ROOT/README.md" "$SCRATCH/README.md"
printf '\nThis trading system is guaranteed profitable and mainnet proven.\n' >> "$SCRATCH/README.md"

if "$SCRATCH/scripts/verify-marketing-claims.sh" >/dev/null 2>&1; then
  echo "[claims-test] FAIL — the gate did NOT reject the planted banned phrases" >&2
  exit 1
else
  echo "[claims-test] planted banned phrases rejected — the gate fails closed as required"
fi

echo "[claims-test] PASS — real tree clean and the gate is proven to reject violations"
exit 0
