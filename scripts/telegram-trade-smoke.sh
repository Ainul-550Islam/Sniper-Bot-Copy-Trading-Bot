#!/usr/bin/env bash
# BATCH 04 — end-to-end smoke for Telegram `/buy` `/sell` + trade consent.
#
# What it proves, in order, with no mocks beyond the crate's own fake Bot API:
#
#   1. the crate builds and its inline (`--lib`) tests pass;
#   2. the real dispatcher routing table answers the new commands and still
#      answers the old ones;
#   3. a full buy and a full sell run through the REAL dispatcher against the
#      fake Telegram server and the in-memory fake `TradeCommandPort`
#      (`requested` -> `confirmed` -> `executed`, exactly one port call);
#   4. no audit row and no chat reply leaks the nonce or the bot token;
#   5. the two migrations carry the required guards (RLS enabled AND forced,
#      `app.current_org_id()` guard, role-existence-guarded GRANTs, idempotent
#      DDL);
#   6. when a database is reachable the two migrations really apply twice on
#      top of the whole graph and leave the expected tables/policies behind.
#
# The database half is honest about absence: with `REQUIRE_POSTGRES=1` and no
# `POSTGRES_URL` it FAILS (a missing database may never look like a green run);
# without `REQUIRE_POSTGRES` it prints SKIP and exits non-zero only if the DDL
# checks fail.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Build single-threaded unless the caller says otherwise. This script compiles
# the module and its test targets; on a memory-constrained runner (or CI with a
# small cgroup limit) two concurrent rustc processes are enough to get one of
# them OOM-killed, which would look like a test failure instead of a resource
# limit. An explicit CARGO_BUILD_JOBS always wins.
: "${CARGO_BUILD_JOBS:=1}"
export CARGO_BUILD_JOBS

MIGRATIONS=(
    "crates/core/migrations/0066_telegram_trade_consents.sql"
    "crates/core/migrations/0067_telegram_trade_commands.sql"
)
FAILED=0

step() { printf '\n=== %s\n' "$*"; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    FAILED=1
}
ok() { printf 'ok: %s\n' "$*"; }

# ---------------------------------------------------------------------------
step "1. build + inline tests"
# ---------------------------------------------------------------------------
if ! cargo test -q -p module-telegram --lib; then
    fail "module-telegram --lib tests"
else
    ok "inline tests"
fi

# ---------------------------------------------------------------------------
step "2/3/4. dispatcher + full buy/sell flow + redaction"
# ---------------------------------------------------------------------------
for test_target in buy_sell_flow audit_redaction existing_commands_unchanged; do
    if ! cargo test -q -p module-telegram --test "$test_target"; then
        fail "integration target $test_target"
    else
        ok "$test_target"
    fi
done

# ---------------------------------------------------------------------------
step "5. migration guards"
# ---------------------------------------------------------------------------
for migration in "${MIGRATIONS[@]}"; do
    if [[ ! -f "$migration" ]]; then
        fail "missing migration $migration"
        continue
    fi
    require_grep() {
        if ! grep -Eq "$1" "$migration"; then
            fail "$migration is missing: $2"
        fi
    }
    require_grep 'CREATE TABLE IF NOT EXISTS' "idempotent CREATE TABLE"
    require_grep 'CREATE (UNIQUE )?INDEX IF NOT EXISTS' "idempotent CREATE INDEX"
    require_grep 'ENABLE ROW LEVEL SECURITY' "ENABLE ROW LEVEL SECURITY"
    require_grep 'FORCE ROW LEVEL SECURITY' "FORCE ROW LEVEL SECURITY"
    require_grep "to_regprocedure\('app.current_org_id\(\)'\) IS NOT NULL" \
        "guard on app.current_org_id()"
    require_grep "CREATE POLICY tenant_org_isolation" "tenant_org_isolation policy"
    require_grep 'USING \(organization_id = app.current_org_id\(\)\)' "USING tenant predicate"
    require_grep 'WITH CHECK \(organization_id = app.current_org_id\(\)\)' "WITH CHECK tenant predicate"
    require_grep "pg_roles WHERE rolname = 'bot_app'" "role-guarded GRANT (bot_app)"
    require_grep "pg_roles WHERE rolname = 'bot_system'" "role-guarded GRANT (bot_system)"
    if grep -Eqi '(nonce|phrase)[^_]*(text|varchar)' "$migration"; then
        fail "$migration appears to store a plaintext nonce or phrase"
    fi
    ok "$(basename "$migration") guards"
done
if ! grep -q 'nonce_sha256 *bytea' "${MIGRATIONS[1]}"; then
    fail "0067 must store the nonce digest only (nonce_sha256 bytea)"
fi
if ! grep -q 'phrase_sha256 *bytea' "${MIGRATIONS[0]}"; then
    fail "0066 must store the phrase digest only (phrase_sha256 bytea)"
fi

# ---------------------------------------------------------------------------
step "6. apply the migrations to a real database (optional)"
# ---------------------------------------------------------------------------
URL="${POSTGRES_URL:-}"
if [[ -z "$URL" ]]; then
    if [[ "${REQUIRE_POSTGRES:-0}" == "1" ]]; then
        fail "REQUIRE_POSTGRES=1 but POSTGRES_URL is missing"
    else
        printf 'SKIP: POSTGRES_URL unset — database half not exercised\n'
        printf '      (run with POSTGRES_URL=… REQUIRE_POSTGRES=1 to enforce it)\n'
    fi
else
    python3 - "$URL" "$ROOT" <<'PY'
import sys
import psycopg2

url, root = sys.argv[1], sys.argv[2]
migrations = [
    "crates/core/migrations/0066_telegram_trade_consents.sql",
    "crates/core/migrations/0067_telegram_trade_commands.sql",
]

# Everything happens inside ONE transaction that is rolled back: DDL in
# PostgreSQL is transactional, so the smoke run proves the migrations apply
# twice on a real server without leaving a single row or index behind.
conn = psycopg2.connect(url)
conn.autocommit = False
cur = conn.cursor()

def file_sql(path):
    with open(f"{root}/{path}", encoding="utf-8") as handle:
        return handle.read()

for _ in range(2):
    for path in migrations:
        cur.execute(file_sql(path))

cur.execute("SELECT to_regprocedure('app.current_org_id()') IS NOT NULL")
foundation = cur.fetchone()[0]

cur.execute(
    """
    SELECT relname, relrowsecurity, relforcerowsecurity
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE n.nspname = 'public'
       AND c.relname IN ('telegram_trade_consents','telegram_trade_commands',
                         'telegram_trade_audit')
     ORDER BY relname
    """
)
rows = cur.fetchall()
assert len(rows) == 3, f"expected 3 tables, got {rows}"
if foundation:
    # The Batch 01 foundation exists, so the guarded block must have fired:
    # every table has RLS ENABLED *and* FORCED.
    for name, rls, force in rows:
        assert rls, f"{name} has no RLS"
        assert force, f"{name} does not FORCE RLS"
    print("ok: three tables, RLS enabled and forced (migrations applied twice)")
    cur.execute(
        """
        SELECT tablename FROM pg_policies
         WHERE schemaname = 'public' AND tablename LIKE 'telegram_trade_%'
           AND policyname = 'tenant_org_isolation'
         ORDER BY tablename
        """
    )
    policies = [row[0] for row in cur.fetchall()]
    assert len(policies) == 3, f"expected the policy on all three tables, got {policies}"
    print("ok: tenant_org_isolation on all three tables")
else:
    # No foundation: the guard must stay inert (fail-open on DDL, never on
    # data) — this is what makes the migration safe to run before Batch 01.
    for name, rls, _force in rows:
        assert not rls, (
            f"{name} has RLS without app.current_org_id()); "
            "the guard must skip the policy, not half-install it"
        )
    print("ok: three tables; tenant guard correctly inert (no app.current_org_id())")

cur.execute(
    """
    SELECT indexname FROM pg_indexes
     WHERE schemaname = 'public' AND tablename LIKE 'telegram_trade_%'
       AND indexname IN ('telegram_trade_consents_one_active',
                         'telegram_trade_commands_one_pending')
    """
)
found = sorted(row[0] for row in cur.fetchall())
assert found == ['telegram_trade_commands_one_pending',
                 'telegram_trade_consents_one_active'], found
print("ok: one-live-consent and one-pending-command indexes exist")

conn.rollback()
conn.close()
PY
    if [[ $? -ne 0 ]]; then
        fail "database migration smoke"
    else
        ok "database migrations"
    fi
fi

# ---------------------------------------------------------------------------
if (( FAILED )); then
    printf '\ntelegram-trade-smoke: FAIL\n' >&2
    exit 1
fi
printf '\ntelegram-trade-smoke: PASS\n'
