#!/usr/bin/env bash
# Fail the build when production control-plane surfaces reintroduce the
# canned rows, money, identities, or unsupported security claims that this
# repository explicitly forbids. Test fixtures may contain examples, so the
# scan is limited to runtime server and browser source directories.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGETS=(
  "$ROOT/crates/server/src/saas"
  "$ROOT/crates/server/src/trading_data_plane"
  "$ROOT/apps/control-plane/src/app"
  "$ROOT/apps/control-plane/src/components"
  "$ROOT/apps/control-plane/src/lib"
)
PATTERN='act-01|alt-01|tkt-2026-9481|acme-quant|4,850,290|4_850_290|4,850,000|4_850_000|185\.5|1,420,000|1_420_000|198\.51\.100|203\.0\.113|FIPS[[:space:]]+140|SOC[[:space:]]*2|SOC2|HSM|sub-millisecond|zero-latency|Connected[[:space:]]*&[[:space:]]*Verified'

if grep -RInE --exclude='*.map' --exclude-dir='__tests__' --exclude-dir='tests' "$PATTERN" "${TARGETS[@]}"; then
  echo "forbid-fake-data: forbidden canned data or unsupported claim found" >&2
  exit 1
fi

echo "forbid-fake-data: no forbidden production literals found"
