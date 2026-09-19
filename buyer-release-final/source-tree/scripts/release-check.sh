#!/usr/bin/env bash
# ============================================================================
# release-check.sh — local release validation gate.
#
# Runs every check that does not require external network resources or a
# Docker daemon. Gated integration suites EXECUTE when POSTGRES_URL /
# REDIS_URL are present and report SKIP (not failure) when they are not —
# network-gated tests (devnet, validator e2e, latency bench) are never
# mandatory here by design.
#
# Usage:   ./scripts/release-check.sh
# Exit:    0 = every executed step passed; 1 = at least one FAIL.
# ============================================================================
set -u

cd "$(dirname "$0")/.." || exit 1
ROOT="$PWD"

PASS=0; FAIL=0; SKIP=0
FAILED_STEPS=()

step() { # step <name> <command...>
    local name="$1"; shift
    printf '\n=== %s ===\n' "$name"
    if "$@"; then
        printf '[PASS] %s\n' "$name"; PASS=$((PASS+1))
    else
        printf '[FAIL] %s\n' "$name"; FAIL=$((FAIL+1)); FAILED_STEPS+=("$name")
    fi
}

skip() { # skip <name> <reason>
    printf '\n=== %s ===\n[SKIP] %s — %s\n' "$1" "$1" "$2"
    SKIP=$((SKIP+1))
}

# ---------------------------------------------------------------- files ----
required_files=(
    Cargo.toml Cargo.lock VERSION CHANGELOG.md LICENSE SECURITY.md README.md
    AUDIT.md rust-toolchain.toml deny.toml Dockerfile .dockerignore
    docker-compose.yml .env.template config.toml.example .gitignore
    release-manifest.json
    .github/workflows/ci.yml scripts/release-check.sh
    docs/ARCHITECTURE.md docs/API.md docs/SECURITY.md docs/DEPLOYMENT.md
    docs/OPERATIONS.md docs/MODULES.md docs/STAKING.md docs/TESTING.md
    docs/RECONCILIATION.md docs/DISTRIBUTED.md docs/RELEASE.md
    docs/HANDOVER.md docs/BACKUP-RESTORE.md
)
missing=0
for f in "${required_files[@]}"; do
    [ -f "$f" ] || { echo "missing required file: $f"; missing=1; }
done
step "required release files present" test "$missing" -eq 0

# ------------------------------------------------------------- versions ----
version_check() {
    local v_file v_ws v_stk v_man
    v_file="$(tr -d '[:space:]' < VERSION)"
    v_ws="$(awk -F'"' '/^\[workspace.package\]/{f=1;next} f&&/^version/{print $2;exit}' Cargo.toml)"
    v_stk="$(awk -F'"' '/^version/{print $2;exit}' programs/staking-suite/Cargo.toml)"
    # First "version" key in the manifest is the product version (manifest_version
    # is a differently-named key, so this grep is unambiguous).
    v_man="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]+"' release-manifest.json | head -1 | sed 's/.*"\([^"]*\)"$/\1/')"
    echo "VERSION=$v_file workspace=$v_ws staking=$v_stk manifest=$v_man"
    [ -n "$v_file" ] && [ "$v_file" = "$v_ws" ] && [ "$v_ws" = "$v_stk" ] && [ "$v_stk" = "$v_man" ]
}
step "version consistency (VERSION == workspace == staking)" version_check

toolchain_check() {
    local pin docker ci
    pin="$(awk -F'"' '/^channel/{print $2;exit}' rust-toolchain.toml)"
    docker="$(grep -oE 'FROM rust:[0-9.]+-bookworm' Dockerfile | head -1 | sed 's/FROM rust://; s/-bookworm//')"
    ci="$(grep -oE 'dtolnay/rust-toolchain@[0-9.]+' .github/workflows/ci.yml | sed 's|.*@||' | sort -u | tr '\n' ' ')"
    echo "pin=$pin dockerfile=$docker ci-explicit=[$ci]"
    [ -n "$pin" ] && [ "$docker" = "$pin" ] && case " $ci " in *" $pin "*) true;; *) false;; esac
}
step "toolchain pin consistency (rust-toolchain == Dockerfile == CI program job)" toolchain_check

# ------------------------------------------------------------ migrations ---
migration_check() {
    local files prev n
    files="$(ls crates/core/migrations/*.sql | xargs -n1 basename | sort)"
    prev=0
    while read -r f; do
        n="${f%%_*}"
        [ "$n" -gt "$prev" ] 2>/dev/null || { echo "non-monotonic migration: $f (after $prev)"; return 1; }
        prev="$n"
    done <<< "$files"
    echo "$(echo "$files" | wc -l) migrations, monotonic 0001..$(printf '%04d' "$prev")"
}
step "migrations monotonic + uniquely versioned" migration_check

# --------------------------------------------------------------- markers ---
marker_scan() {
    local hits
    hits="$(grep -rn -E 'TODO|FIXME|todo!\(|unimplemented!\(' \
        --include='*.rs' --include='*.sql' --include='*.toml' \
        --include='*.yml' --include='Dockerfile' . | wc -l)"
    echo "TODO/FIXME/todo!/unimplemented! in code/config files: $hits"
    [ "$hits" -eq 0 ]
}
step "no TODO/FIXME/stub markers in code or config" marker_scan

secret_scan() {
    # Forbidden: real key material. Allowed: env-var NAMES, docs, redaction
    # logic, explicit test fakes. We search for things that look like
    # committed secrets: base58 solana keypair JSON arrays, hex private
    # keys assigned inline, bot tokens (digits:alnum pattern).
    local hits=0
    if grep -rn -E '(bot_token|api_key|secret|private_key|password)[[:space:]]*=[[:space:]]*"[A-Za-z0-9_\-]{20,}"' \
        --include='*.rs' --include='*.toml' --include='*.yml' --include='*.json' . \
        | grep -v -E 'example|template|test|change-me|\.lock'; then
        hits=1
    fi
    if grep -rn -E '[0-9]{8,10}:[A-Za-z0-9_\-]{30,}' --include='*.toml' --include='*.yml' --include='*.rs' . ; then
        hits=1
    fi
    [ "$hits" -eq 0 ]
}
step "no secret-looking literals committed" secret_scan

# ------------------------------------------------------------ rust gates ---
step "cargo fmt --all --check" cargo fmt --all --check
step "cargo check --workspace" cargo check --workspace
step "cargo clippy --workspace --all-targets -- -D warnings" \
    cargo clippy --workspace --all-targets -- -D warnings

# --test-threads=1 mirrors the CI `test` step: the db_integration suite
# shares one database (audit-chain tests verify global state), so parallel
# test threads inside that binary would race the shared chain.
step "cargo test --workspace (gated suites run iff env present)" \
    cargo test --workspace -- --test-threads=1

if [ -n "${POSTGRES_URL:-}" ]; then
    step "db_integration (real Postgres)" \
        cargo test -p bot-core --test db_integration -- --test-threads=1
else
    skip "db_integration" "POSTGRES_URL not set"
fi
if [ -n "${REDIS_URL:-}" ]; then
    step "redis_integration (real Redis)" \
        cargo test -p bot-core --test redis_integration -- --test-threads=1
else
    skip "redis_integration" "REDIS_URL not set"
fi
if [ -n "${POSTGRES_URL:-}" ] && [ -n "${REDIS_URL:-}" ]; then
    step "distributed_integration (real Postgres + Redis)" \
        cargo test -p bot-core --test distributed_integration -- --test-threads=1
else
    skip "distributed_integration" "POSTGRES_URL and/or REDIS_URL not set"
fi
if [ -n "${POSTGRES_URL:-}" ]; then
    step "two_replica_mirror (real Postgres)" \
        cargo test -p module-copy --test two_replica_mirror -- --test-threads=1
else
    skip "two_replica_mirror" "POSTGRES_URL not set"
fi

# ---------------------------------------------------------------- staking --
staking_fmt()   { ( cd programs/staking-suite && cargo fmt --check ); }
staking_clippy(){ ( cd programs/staking-suite && cargo clippy --all-targets -- -D warnings ); }
staking_test()  { ( cd programs/staking-suite && cargo test ); }
step "staking: cargo fmt --check" staking_fmt
step "staking: cargo clippy --all-targets -- -D warnings" staking_clippy
step "staking: cargo test (host; validator e2e gated on STAKING_E2E)" staking_test

# ------------------------------------------------------- supply chain ------
step "cargo audit (app lockfile)" cargo audit
staking_audit() { ( cd programs/staking-suite && cargo audit ); }
step "cargo audit (staking lockfile)" staking_audit
step "cargo deny check" cargo deny check

# ---------------------------------------------------------------- summary --
printf '\n============================================================\n'
printf 'release-check summary: %d PASS, %d FAIL, %d SKIP\n' "$PASS" "$FAIL" "$SKIP"
if [ "$FAIL" -gt 0 ]; then
    printf 'FAILED steps:\n'; for s in "${FAILED_STEPS[@]}"; do printf '  - %s\n' "$s"; done
    exit 1
fi
printf 'All executed release gates passed.\n'
exit 0
