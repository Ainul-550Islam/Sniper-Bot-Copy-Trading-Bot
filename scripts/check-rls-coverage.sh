#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="batch01-rls-$RANDOM-$$"
MODE=""
PGDATA=""
PORT=""
PG_BINDIR=""

run_psql() {
  if [ "$MODE" = docker ]; then
    docker exec -i "$CONTAINER" psql -U postgres -d postgres "$@"
  else
    psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres "$@"
  fi
}

cleanup() {
  if [ "$MODE" = docker ]; then
    if docker inspect "$CONTAINER" >/dev/null 2>&1; then
      docker rm -f "$CONTAINER" >/dev/null
    fi
  elif [ "$MODE" = local ] && [ -n "$PGDATA" ]; then
    if sudo -n -u postgres "$PG_BINDIR/pg_ctl" -D "$PGDATA" status >/dev/null 2>&1; then
      sudo -n -u postgres "$PG_BINDIR/pg_ctl" -D "$PGDATA" -m immediate stop >/dev/null
    fi
    sudo -n rm -rf "$PGDATA"
  fi
}
trap cleanup EXIT

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  MODE=docker
  docker run --rm -d --name "$CONTAINER" \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    -p 127.0.0.1::5432 postgres:16 >/dev/null
  for attempt in $(seq 1 60); do
    if docker exec "$CONTAINER" pg_isready -U postgres -d postgres >/dev/null 2>&1; then
      break
    fi
    if [ "$attempt" -eq 60 ]; then
      echo "FAIL: PostgreSQL 16 did not become ready" >&2
      exit 1
    fi
    sleep 1
  done
  ENDPOINT="Docker PostgreSQL 16"
else
  PG_BINDIR="${PG_BINDIR:-$(pg_config --bindir 2>/dev/null || true)}"
  if [ -x "$PG_BINDIR/initdb" ] && [ -x "$PG_BINDIR/pg_ctl" ] \
    && command -v sudo >/dev/null 2>&1 && sudo -n -u postgres true >/dev/null 2>&1; then
    MODE=local
    PGDATA="$(mktemp -d /tmp/batch01-postgres.XXXXXX)"
    sudo -n chown postgres:postgres "$PGDATA"
    PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
    sudo -n -u postgres "$PG_BINDIR/initdb" -D "$PGDATA" \
      --username=postgres --auth-local=trust --auth-host=trust >/dev/null
    sudo -n -u postgres "$PG_BINDIR/pg_ctl" -D "$PGDATA" \
      -o "-h 127.0.0.1 -p $PORT" -w start >/dev/null
    PG_MAJOR="$("$PG_BINDIR/postgres" --version | awk '{print $3}' | cut -d. -f1)"
    ENDPOINT="local PostgreSQL $PG_MAJOR on port $PORT"
  else
    echo "FAIL: Docker or a local PostgreSQL installation with initdb/pg_ctl is required" >&2
    exit 1
  fi
fi

for migration in "$ROOT"/crates/core/migrations/*.sql; do
  run_psql -v ON_ERROR_STOP=1 < "$migration" >/dev/null
done
# Applying the RLS foundation twice must be safe and leave the same policies.
for migration in 0060_tenant_rls_foundation.sql 0061_tenant_rls_exempt_registry.sql 0062_tenant_rls_policies.sql; do
  run_psql -v ON_ERROR_STOP=1 < "$ROOT/crates/core/migrations/$migration" >/dev/null
done

run_psql -v ON_ERROR_STOP=1 <<'SQL'
DO $$
DECLARE missing_count integer;
BEGIN
  SELECT count(*) INTO missing_count
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
   WHERE n.nspname = 'public'
     AND c.relkind IN ('r', 'p')
     AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
     AND NOT c.relrowsecurity
     AND NOT EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = c.relname);
  IF missing_count <> 0 THEN
    RAISE EXCEPTION 'tables missing ENABLE ROW LEVEL SECURITY: %', missing_count;
  END IF;
  SELECT count(*) INTO missing_count
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
   WHERE n.nspname = 'public'
     AND c.relkind IN ('r', 'p')
     AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
     AND NOT c.relforcerowsecurity
     AND NOT EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = c.relname);
  IF missing_count <> 0 THEN
    RAISE EXCEPTION 'tables missing FORCE ROW LEVEL SECURITY: %', missing_count;
  END IF;
  SELECT count(*) INTO missing_count
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
   WHERE n.nspname = 'public'
     AND c.relkind IN ('r', 'p')
     AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
     AND NOT EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = c.relname)
     AND NOT EXISTS (
       SELECT 1 FROM pg_policies p
        WHERE p.schemaname = n.nspname
          AND p.tablename = c.relname
          AND p.policyname = 'tenant_org_isolation'
     );
  IF missing_count <> 0 THEN
    RAISE EXCEPTION 'tenant-keyed tables missing tenant_org_isolation policy: %', missing_count;
  END IF;
END $$;
SQL
printf 'RLS coverage PASS (%s; migrations applied twice)\n' "$ENDPOINT"
