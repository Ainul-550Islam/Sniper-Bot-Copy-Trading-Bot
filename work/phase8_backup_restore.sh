#!/usr/bin/env bash
# Phase 8 — real pg_dump -> restore -> re-verify round-trip on PostgreSQL 17.11.
# Never touches the `postgres` database used by other gates.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
EV=/home/user/evidence
REPO=/home/user/sniper-suite
SRC=sniper_phase8_src
RST=sniper_phase8_restore
PSQL="psql -h 127.0.0.1 -p 5432 -U postgres -v ON_ERROR_STOP=1 -At"
STAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

echo "== phase8 start $STAMP =="
dropdb -h 127.0.0.1 -U postgres --if-exists "$SRC"
dropdb -h 127.0.0.1 -U postgres --if-exists "$RST"
createdb -h 127.0.0.1 -U postgres "$SRC"

echo "== 1. populate SRC via db_integration (applies migrations 0001-0011 + writes real rows) =="
cd "$REPO"
POSTGRES_URL="postgres://postgres@127.0.0.1:5432/$SRC" REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p bot-core --test db_integration -j 1 -- --test-threads=1 2>&1 | tee "$EV/phase8-1-populate.log" | tail -4

echo "== 2. snapshot SRC state =="
$PSQL -d "$SRC" -c "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY 1" > "$EV/phase8-src-tables.txt"
$PSQL -d "$SRC" -c "SELECT version, description, checksum FROM _sqlx_migrations ORDER BY version" > "$EV/phase8-src-migrations.txt"
while read -r t; do
  n=$($PSQL -d "$SRC" -c "SELECT count(*) FROM \"$t\"")
  echo "$t=$n"
done < "$EV/phase8-src-tables.txt" > "$EV/phase8-src-rowcounts.txt"
wc -l < "$EV/phase8-src-tables.txt" | xargs echo "tables:"
cat "$EV/phase8-src-rowcounts.txt" | awk -F= '{s+=$2} END {print "total rows:", s}'

echo "== 3. pg_dump -Fc =="
pg_dump -h 127.0.0.1 -U postgres -Fc "$SRC" > "$EV/backup-$STAMP.dump"
ls -la "$EV/backup-$STAMP.dump"
sha256sum "$EV/backup-$STAMP.dump" | tee "$EV/phase8-3-dump.sha256"
[ -s "$EV/backup-$STAMP.dump" ] || { echo "DUMP EMPTY"; exit 1; }

echo "== 4. restore into fresh DB =="
createdb -h 127.0.0.1 -U postgres "$RST"
pg_restore -h 127.0.0.1 -U postgres -d "$RST" --no-owner "$EV/backup-$STAMP.dump" 2>&1 | tee "$EV/phase8-4-restore.log" || true
grep -i error "$EV/phase8-4-restore.log" && { echo "RESTORE ERRORS"; exit 1; } || echo "restore: no errors"

echo "== 5. compare schema + row counts + migration state =="
$PSQL -d "$RST" -c "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY 1" > "$EV/phase8-rst-tables.txt"
$PSQL -d "$RST" -c "SELECT version, description, checksum FROM _sqlx_migrations ORDER BY version" > "$EV/phase8-rst-migrations.txt"
while read -r t; do
  n=$($PSQL -d "$RST" -c "SELECT count(*) FROM \"$t\"")
  echo "$t=$n"
done < "$EV/phase8-rst-tables.txt" > "$EV/phase8-rst-rowcounts.txt"
diff "$EV/phase8-src-tables.txt" "$EV/phase8-rst-tables.txt" && echo "TABLES_IDENTICAL"
diff "$EV/phase8-src-migrations.txt" "$EV/phase8-rst-migrations.txt" && echo "MIGRATIONS_IDENTICAL"
diff "$EV/phase8-src-rowcounts.txt" "$EV/phase8-rst-rowcounts.txt" && echo "ROWCOUNTS_IDENTICAL"

echo "== 6. db_integration re-run AGAINST RESTORED DB (proves schema+data usable, audit chain verifies) =="
POSTGRES_URL="postgres://postgres@127.0.0.1:5432/$RST" REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p bot-core --test db_integration -j 1 -- --test-threads=1 2>&1 | tee "$EV/phase8-6-restored-rerun.log" | tail -4

echo "== 7. application startup against restored DB (native debug binary; Docker blocked in sandbox) =="
cd "$REPO"
cargo build --bin sniper-suite -j 1 2>&1 | tail -1
mkdir -p /home/user/work/app-run && cd /home/user/work/app-run
cp "$REPO/config.toml.example" ./config.toml
export CONFIG_PATH=/home/user/work/app-run/config.toml
export POSTGRES_URL="postgres://postgres@127.0.0.1:5432/$RST"
export REDIS_URL=redis://127.0.0.1:6379
export API_KEY=phase8-local-test-key
export RUST_LOG=info
"$REPO/target/debug/sniper-suite" > "$EV/phase8-7-app.log" 2>&1 &
APP_PID=$!
ok=0
for i in $(seq 1 45); do
  if curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then ok=1; break; fi
  sleep 1
done
[ "$ok" = 1 ] || { echo "APP FAILED TO START"; tail -30 "$EV/phase8-7-app.log"; kill $APP_PID 2>/dev/null || true; exit 1; }
echo "--- /health:"  | tee -a "$EV/phase8-7-endpoints.log"; curl -s http://127.0.0.1:8080/health | tee -a "$EV/phase8-7-endpoints.log"; echo | tee -a "$EV/phase8-7-endpoints.log"
echo "--- /ready:"   | tee -a "$EV/phase8-7-endpoints.log"; curl -si http://127.0.0.1:8080/ready | head -12 | tee -a "$EV/phase8-7-endpoints.log"
echo "--- /api/health:" | tee -a "$EV/phase8-7-endpoints.log"; curl -s http://127.0.0.1:8080/api/health | tee -a "$EV/phase8-7-endpoints.log"; echo | tee -a "$EV/phase8-7-endpoints.log"
echo "--- /api/status:" | tee -a "$EV/phase8-7-endpoints.log"; curl -s http://127.0.0.1:8080/api/status | head -c 600 | tee -a "$EV/phase8-7-endpoints.log"; echo | tee -a "$EV/phase8-7-endpoints.log"
echo "--- /api/audit/verify:" | tee -a "$EV/phase8-7-endpoints.log"; curl -s http://127.0.0.1:8080/api/audit/verify -H "x-api-key: $API_KEY" | tee -a "$EV/phase8-7-endpoints.log"; echo | tee -a "$EV/phase8-7-endpoints.log"
echo "--- /metrics (first bot_ lines):" | tee -a "$EV/phase8-7-endpoints.log"; curl -s http://127.0.0.1:8080/metrics | grep '^bot_' | head -8 | tee -a "$EV/phase8-7-endpoints.log"

echo "== 8. graceful shutdown (SIGTERM) =="
kill -TERM $APP_PID
for i in $(seq 1 30); do kill -0 $APP_PID 2>/dev/null || break; sleep 1; done
if kill -0 $APP_PID 2>/dev/null; then echo "APP DID NOT EXIT AFTER SIGTERM"; kill -9 $APP_PID; exit 1; fi
echo "app exited after SIGTERM"
tail -15 "$EV/phase8-7-app.log" | tee -a "$EV/phase8-7-shutdown.log"
grep -iE 'shutdown|graceful|stopped|exiting' "$EV/phase8-7-app.log" | tail -6 || true
echo "== phase8 DONE $(date -u +%Y-%m-%dT%H:%M:%SZ) =="
