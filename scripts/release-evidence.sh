#!/usr/bin/env bash
# release-evidence.sh — one honest release report from the current source SHA.
#
# Every status comes from a real command's exit code, never from a hard-coded
# string:
#   PASS    — the command ran and exited 0 (and did not print NOT_RUN)
#   FAIL    — the command ran and exited non-zero
#   NOT_RUN — the tool, service or env var needed to run it is absent
#   BLOCK   — a release prerequisite is missing (for example buyer-release/)
#   WARN    — a non-blocking finding (for example a stale phrase in docs)
# The overall verdict is BLOCK if any check is FAIL or BLOCK, otherwise
# NOT_RUN if any check is NOT_RUN, otherwise WARN if any is WARN, else PASS.
# Output: release-evidence/{summary.json,commands.txt,logs/*.txt}.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/release-evidence"
LOGS="$OUT/logs"
RESULTS="$OUT/.results.tsv"

rm -rf "$OUT"
mkdir -p "$LOGS"
: > "$RESULTS"
: > "$OUT/commands.txt"

log_cmd() { printf '%s\n' "$*" >> "$OUT/commands.txt"; }

# record <name> <status> <detail>
record() {
  printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$RESULTS"
  echo "[release-evidence] $1: $2 — $3"
}

# run_check <name> <log-file> <cmd...>
# NOT_RUN when the first word of cmd is not installed; otherwise exit-code based.
run_check() {
  local name="$1" log="$2"; shift 2
  if ! command -v "$1" >/dev/null 2>&1; then
    record "$name" NOT_RUN "$1 not installed in this environment"
    return
  fi
  log_cmd "$*"
  "$@" > "$log" 2>&1
  local rc=$?
  if [ "$rc" -eq 0 ]; then
    record "$name" PASS "exit 0: $*"
  else
    record "$name" FAIL "exit $rc: $*"
  fi
}

# run_env_check <name> <env-var> <log-file> <cmd...>
# NOT_RUN when the env var is unset. Tests that print NOT_RUN and return early
# are NOT counted as PASS.
run_env_check() {
  local name="$1" var="$2" log="$3"; shift 3
  if [ -z "${!var:-}" ]; then
    record "$name" NOT_RUN "$var is not set; integration suite not executed"
    return
  fi
  log_cmd "$*"
  "$@" > "$log" 2>&1
  local rc=$?
  if grep -q "NOT_RUN" "$log"; then
    record "$name" NOT_RUN "suite printed NOT_RUN (exit $rc)"
  elif [ "$rc" -eq 0 ]; then
    record "$name" PASS "exit 0: $*"
  else
    record "$name" FAIL "exit $rc: $*"
  fi
}

# --- source identity ---------------------------------------------------------
SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY="$(git -C "$ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION" 2>/dev/null || echo unknown)"
{
  echo "# release-evidence commands"
  echo "date_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "version: $VERSION"
  echo "source_sha: $SHA"
  echo "uncommitted_entries: $DIRTY"
} > "$OUT/commands.txt"

# --- Rust --------------------------------------------------------------------
cd "$ROOT" || exit 1
run_check format   "$LOGS/rust-fmt.txt"    cargo fmt --all -- --check
run_check check    "$LOGS/rust-check.txt"  cargo check --workspace --all-targets
run_check clippy   "$LOGS/rust-clippy.txt" cargo clippy --workspace --all-targets -- -D warnings
run_check rust_test "$LOGS/rust-test.txt"  cargo test --workspace -- --test-threads=1
run_env_check postgres_integration POSTGRES_URL "$LOGS/postgres-integration.txt" \
  cargo test --test postgres_saas_integration -- --nocapture
run_env_check redis_integration REDIS_URL "$LOGS/redis-integration.txt" \
  cargo test --test redis_saas_integration -- --nocapture

# --- frontend ----------------------------------------------------------------
FE="$ROOT/apps/control-plane"
if [ ! -f "$FE/package.json" ]; then
  record frontend NOT_RUN "apps/control-plane/package.json missing"
elif ! command -v npm >/dev/null 2>&1; then
  record frontend NOT_RUN "npm not installed in this environment"
else
  for step in "ci:npm ci" "typecheck:npm run typecheck" "lint:npm run lint" \
              "test:npm test" "build:npm run build"; do
    name="${step%%:*}"; cmd="${step#*:}"
    log="$LOGS/frontend-$name.txt"
    log_cmd "(cd apps/control-plane && $cmd)"
    # shellcheck disable=SC2086
    if (cd "$FE" && $cmd) > "$log" 2>&1; then
      record "frontend_$name" PASS "exit 0: $cmd"
    else
      rc=$?
      record "frontend_$name" FAIL "exit $rc: $cmd"
      break
    fi
  done
fi

# --- secrets: canonical scanner is the single source ---------------------------
log_cmd "bash scripts/scan-secrets.sh"
if bash "$ROOT/scripts/scan-secrets.sh" > "$LOGS/secret-scan.txt" 2>&1; then
  record secret_scan PASS "scripts/scan-secrets.sh exit 0"
else
  record secret_scan FAIL "scripts/scan-secrets.sh exit non-zero (see logs/secret-scan.txt)"
fi

# --- marketing / stale claims: the context-aware canonical verifier ----------
run_check claims "$LOGS/marketing-claims.txt" bash scripts/verify-marketing-claims.sh

# --- repo gates (exit-code based) --------------------------------------------
run_check manifest_current "$LOGS/manifest-current.txt" bash tests/release/manifest_current.sh
run_check stats_current    "$LOGS/stats-current.txt"    bash tests/release/stats_current.sh
run_check release_check    "$LOGS/release-check.txt"    bash scripts/release-check.sh

# --- buyer package: a missing package is BLOCK, never PASS -------------------
if [ ! -d "$ROOT/buyer-release" ]; then
  record buyer_package BLOCK "buyer-release/ does not exist; package not built (LICENSE placeholder blocks the build)"
else
  run_check buyer_package "$LOGS/buyer-package.txt" bash scripts/verify-buyer-package.sh
fi
run_check release_integrity "$LOGS/release-integrity.txt" bash scripts/verify-release-integrity.sh

# --- aggregate -----------------------------------------------------------------
python3 - "$RESULTS" "$OUT/summary.json" "$SHA" "$VERSION" "$DIRTY" <<'PY'
import json, sys, datetime
results_path, out_path, sha, version, dirty = sys.argv[1:6]
checks = []
for line in open(results_path, encoding="utf-8"):
    line = line.rstrip("\n")
    if not line:
        continue
    name, status, detail = line.split("\t", 2)
    checks.append({"name": name, "status": status, "detail": detail})
statuses = {c["status"] for c in checks}
if statuses & {"FAIL", "BLOCK"}:
    verdict = "BLOCK"
elif "NOT_RUN" in statuses:
    verdict = "NOT_RUN"
elif "WARN" in statuses:
    verdict = "WARN"
else:
    verdict = "PASS"
summary = {
    "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "version": version,
    "source_sha": sha,
    "uncommitted_entries": int(dirty),
    "verdict": verdict,
    "checks": checks,
}
with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(summary, fh, indent=2)
    fh.write("\n")
print(f"[release-evidence] verdict={verdict} ({len(checks)} checks) -> {out_path}")
PY
rm -f "$RESULTS"
# Exit non-zero unless the verdict is PASS: a report is not a green light.
grep -q '"verdict": "PASS"' "$OUT/summary.json"
