#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/release-evidence"
mkdir -p "$OUT"

echo "[release-evidence] generating at $OUT"

# helper
record_status() {
  local name="$1" status="$2" detail="$3"
  echo "{\"name\":\"$name\",\"status\":\"$status\",\"detail\":\"$detail\"}" >> "$OUT/summary.jsonl"
}

rm -f "$OUT/summary.jsonl" "$OUT/summary.json" "$OUT/manifest.json" "$OUT/commands.txt" "$OUT/checksums.txt"

cat > "$OUT/manifest.json" <<EOF
{"generated_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","version":"$(cat "$ROOT/VERSION" 2>/dev/null || echo unknown)","tool":"release-evidence.sh"}
EOF

{
  echo "# release-evidence commands"
  echo "date: $(date -u)"
  echo "version: $(cat "$ROOT/VERSION" 2>/dev/null || echo unknown)"
  echo "commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
} > "$OUT/commands.txt"

# format check
if cargo fmt --all -- --check > "$OUT/rust-fmt.txt" 2>&1; then
  echo "PASS" > "$OUT/format.txt"
  record_status "format" "PASS" "cargo fmt clean"
else
  echo "FAIL" > "$OUT/format.txt"
  record_status "format" "FAIL" "cargo fmt failed"
fi

# cargo check
if cargo check --workspace > "$OUT/rust-check.txt" 2>&1; then
  record_status "check" "PASS" "cargo check PASS"
else
  record_status "check" "FAIL" "cargo check FAIL"
fi

# clippy where available
if cargo clippy --workspace --all-targets -- -D warnings > "$OUT/rust-clippy.txt" 2>&1; then
  record_status "clippy" "PASS" "clippy clean"
else
  # don't fail overall if clippy not installed, mark WARN
  if grep -q "error: no such command" "$OUT/rust-clippy.txt"; then
    record_status "clippy" "WARN" "clippy not installed"
  else
    record_status "clippy" "FAIL" "clippy warnings"
  fi
fi

# unit tests
if cargo test --workspace -- --test-threads=1 > "$OUT/rust-test.txt" 2>&1; then
  record_status "rust_test" "PASS" "workspace tests PASS"
else
  record_status "rust_test" "FAIL" "workspace tests FAIL"
fi

# postgres integration (NOT_RUN if missing)
if cargo test --test postgres_saas_integration -- --nocapture > "$OUT/postgres-integration.txt" 2>&1; then
  if grep -q "NOT_RUN" "$OUT/postgres-integration.txt"; then
    record_status "postgres" "NOT_RUN" "POSTGRES_URL missing"
  else
    record_status "postgres" "PASS" "postgres integration PASS"
  fi
else
  if grep -q "NOT_RUN" "$OUT/postgres-integration.txt"; then
    record_status "postgres" "NOT_RUN" "postgres NOT_RUN"
  else
    record_status "postgres" "FAIL" "postgres FAIL"
  fi
fi

# redis integration
if cargo test --test redis_saas_integration -- --nocapture > "$OUT/redis-integration.txt" 2>&1; then
  if grep -q "NOT_RUN" "$OUT/redis-integration.txt"; then
    record_status "redis" "NOT_RUN" "REDIS_URL missing"
  else
    record_status "redis" "PASS" "redis PASS"
  fi
else
  if grep -q "NOT_RUN" "$OUT/redis-integration.txt"; then record_status "redis" "NOT_RUN" "redis NOT_RUN"; else record_status "redis" "FAIL" "redis FAIL"; fi
fi

# frontend where available
if [ -d "$ROOT/apps/control-plane" ]; then
  (cd "$ROOT/apps/control-plane" && npm ci --ignore-scripts > "$OUT/frontend-ci.txt" 2>&1 && npm run typecheck > "$OUT/frontend-typecheck.txt" 2>&1 && npm run build > "$OUT/frontend-build.txt" 2>&1)
  if grep -q "error" "$OUT/frontend-typecheck.txt" || grep -q "Failed" "$OUT/frontend-build.txt"; then
    record_status "frontend" "FAIL" "typecheck/build FAIL"
  else
    record_status "frontend" "PASS" "typecheck+build PASS"
  fi
  cat "$OUT/frontend-ci.txt" "$OUT/frontend-typecheck.txt" "$OUT/frontend-build.txt" > "$OUT/frontend.txt" 2>&1 || true
else
  record_status "frontend" "NOT_RUN" "no frontend"
fi

# secret scan
if grep -R --include="*.rs" --include="*.md" --include="*.toml" -n "BEGIN PRIVATE KEY\|sk_live\|postgres://.*:.*@" "$ROOT/crates" "$ROOT/docs" 2>/dev/null | grep -v "<redacted>" | grep -v "test" > "$OUT/secret-scan.txt" 2>&1; then
  if [ -s "$OUT/secret-scan.txt" ]; then record_status "secret_scan" "FAIL" "potential secret found"; else echo "clean" > "$OUT/secret-scan.txt"; record_status "secret_scan" "PASS" "no plaintext secrets"; fi
else
  echo "clean" > "$OUT/secret-scan.txt"
  record_status "secret_scan" "PASS" "no plaintext secrets"
fi

# stale claims
if cargo run --quiet -p sniper-suite -- --help > /dev/null 2>&1; then true; fi
# simple stale phrase scan
if grep -R --include="*.md" -n "11 migrations\|production verified\|live trading verified" "$ROOT/docs" 2>/dev/null | grep -v "HISTORICAL" > "$OUT/stale-claims.txt" 2>&1; then
  if [ -s "$OUT/stale-claims.txt" ]; then record_status "stale_claims" "WARN" "stale phrase without HISTORICAL"; else record_status "stale_claims" "PASS" "no stale claims"; fi
else
  echo "clean" > "$OUT/stale-claims.txt"
  record_status "stale_claims" "PASS" "no stale claims"
fi

# manifest consistency
if "$ROOT/scripts/release-check.sh" > "$OUT/manifest-consistency.txt" 2>&1; then record_status "manifest" "PASS" "release-check PASS"; else record_status "manifest" "FAIL" "release-check FAIL"; fi

# buyer-package verification
if "$ROOT/scripts/verify-buyer-package.sh" > "$OUT/package-verification.txt" 2>&1; then record_status "buyer_package" "PASS" "verify-buyer-package PASS"; else record_status "buyer_package" "FAIL" "verify-buyer-package FAIL"; fi

# checksums
( cd "$ROOT" && find crates docs scripts Cargo.toml Cargo.lock VERSION release-manifest.json -type f -exec sha256sum {} \; 2>/dev/null | head -n 200 > "$OUT/checksums.txt" ) || true

# summary.json aggregate
echo "{" > "$OUT/summary.json"
echo "\"generated_at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"," >> "$OUT/summary.json"
echo "\"version\":\"$(cat "$ROOT/VERSION" 2>/dev/null || echo unknown)\"," >> "$OUT/summary.json"
echo "\"checks\":[" >> "$OUT/summary.json"
if [ -f "$OUT/summary.jsonl" ]; then
  paste -sd, "$OUT/summary.jsonl" >> "$OUT/summary.json"
fi
echo "]" >> "$OUT/summary.json"
echo "}" >> "$OUT/summary.json"

echo "[release-evidence] done at $OUT"
cat "$OUT/summary.json"
