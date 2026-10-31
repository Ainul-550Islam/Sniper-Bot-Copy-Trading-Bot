#!/usr/bin/env bash
# Run the named integration binaries against disposable, real services.
# --existing is used by CI, whose service containers are managed by Actions.
set -euo pipefail
cd "$(dirname "$0")/.."
read -r -a compose <<< "${COMPOSE:-docker compose}"
compose+=( -f docker-compose.test.yml -p sniper-round1-test )
external=0
if [[ ${1:-} == --existing ]]; then
  external=1
  shift
fi
if (( $# )); then echo "usage: $0 [--existing]" >&2; exit 2; fi
log="$(mktemp)"
cleanup() {
  local status=$?
  rm -f "$log"
  if (( ! external )); then "${compose[@]}" down --volumes --remove-orphans || status=1; fi
  exit "$status"
}
trap cleanup EXIT

if (( ! external )); then
  "${compose[@]}" up -d --wait
  export POSTGRES_URL=postgres://sniper:sniper@127.0.0.1:54329/sniper
  export POSTGRES_DDL_URL="$POSTGRES_URL"
  export POSTGRES_APP_URL=postgres://sniper_app:sniper_app_test@127.0.0.1:54329/sniper
  export POSTGRES_SYSTEM_URL=postgres://sniper_system:sniper_system_test@127.0.0.1:54329/sniper
  export REDIS_URL=redis://127.0.0.1:63799
fi
: "${POSTGRES_URL:?POSTGRES_URL required}" "${POSTGRES_DDL_URL:?POSTGRES_DDL_URL required}" \
  "${POSTGRES_APP_URL:?POSTGRES_APP_URL required}" "${POSTGRES_SYSTEM_URL:?POSTGRES_SYSTEM_URL required}" \
  "${REDIS_URL:?REDIS_URL required}"
export REQUIRE_SERVICES=1 REQUIRE_POSTGRES=1

# The migrator creates the group roles. The disposable login principals must
# exist before Database::connect grants them the corresponding memberships.
if (( external )); then
  pg=(psql "$POSTGRES_DDL_URL" -v ON_ERROR_STOP=1)
else
  pg=("${compose[@]}" exec -T postgres psql -U sniper -d sniper -v ON_ERROR_STOP=1)
fi
"${pg[@]}" <<'SQL'
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'sniper_app') THEN
    CREATE ROLE sniper_app LOGIN PASSWORD 'sniper_app_test' NOSUPERUSER NOBYPASSRLS;
  END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'sniper_system') THEN
    CREATE ROLE sniper_system LOGIN PASSWORD 'sniper_system_test' NOSUPERUSER NOBYPASSRLS;
  END IF;
END $$;
SQL

run_test() {
  local package=$1 binary=$2
  shift 2
  : > "$log"
  cargo test --locked -p "$package" --test "$binary" -- --test-threads=1 --nocapture "$@" 2>&1 | tee "$log"
  if ! grep -Eq 'test result: ok\. [1-9][0-9]* passed;' "$log"; then
    echo "FAIL: $binary executed no passing tests" >&2; exit 1
  fi
  if grep -Eq 'NOT_RUN:|skipping redis integration tests' "$log"; then
    echo "FAIL: $binary skipped a required service" >&2; exit 1
  fi
}

# This test applies all embedded SQLx migrations and checks the resulting RLS
# catalog; running it first also makes the schema available to all later tests.
run_test sniper-suite rls_coverage
run_test sniper-suite trusted_proxy_client_ip
# The repository's pre-existing database-specific proxy test is ignored in the
# ordinary suite. Explicitly select it so this service gate really exercises it.
run_test sniper-suite trusted_proxy_client_ip --ignored
run_test sniper-suite rls_isolation
run_test sniper-suite rls_pool_routing
run_test sniper-suite redis_saas_integration
run_test bot-core redis_integration
run_test sniper-suite multi_replica_durable_state
run_test sniper-suite custody_db_authoritative
run_test sniper-suite rls_background_paths
