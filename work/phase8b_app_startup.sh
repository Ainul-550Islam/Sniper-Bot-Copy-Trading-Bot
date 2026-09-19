#!/usr/bin/env bash
# Phase 8 steps 7-8: application startup against the RESTORED database
# (sniper_phase8_restore from phase8-main.log run 2: dump sha256
# 5989ecf1... restored with TABLES/MIGRATIONS/ROWCOUNTS identical and
# db_integration 23/23 green against it), health/readiness checks, audit
# chain verification, graceful shutdown. Native debug binary — Docker is
# not available in this sandbox (recorded separately as BLOCKED).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
EV=/home/user/evidence
REPO=/home/user/sniper-suite
RST=sniper_phase8_restore

cd "$REPO"
echo "== build server binary =="
S=$(date +%s); cargo build --bin sniper-suite -j 1 2>&1 | tail -2; echo "BUILD_EXIT=${PIPESTATUS[0]} DUR=$(( $(date +%s) - S ))s"
ls -la target/debug/sniper-suite

mkdir -p /home/user/work/app-run && cd /home/user/work/app-run
cp "$REPO/config.toml.example" ./config.toml
export CONFIG_PATH=/home/user/work/app-run/config.toml
export POSTGRES_URL="postgres://postgres@127.0.0.1:5432/$RST"
export REDIS_URL=redis://127.0.0.1:6379
export API_KEY=phase8-local-test-key
export RUST_LOG=info
echo "== start app against restored DB =="
"$REPO/target/debug/sniper-suite" > "$EV/phase8b-app.log" 2>&1 &
APP_PID=$!
ok=0
for i in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then ok=1; break; fi
  sleep 1
done
[ "$ok" = 1 ] || { echo "APP FAILED TO START"; tail -30 "$EV/phase8b-app.log"; kill $APP_PID 2>/dev/null || true; exit 1; }
echo "app healthy after ${i}s"
{
  echo "--- GET /health";        curl -s http://127.0.0.1:8080/health; echo
  echo "--- GET /ready (status line + body)"; curl -si http://127.0.0.1:8080/ready | head -14
  echo "--- GET /api/health";    curl -s http://127.0.0.1:8080/api/health; echo
  echo "--- GET /api/status (first 700 bytes)"; curl -s http://127.0.0.1:8080/api/status | head -c 700; echo
  echo "--- GET /api/audit/verify (x-api-key)"; curl -s http://127.0.0.1:8080/api/audit/verify -H "x-api-key: phase8-local-test-key"; echo
  echo "--- GET /metrics (bot_ lines)"; { curl -s http://127.0.0.1:8080/metrics | grep '^bot_' | head -10; } || true
} 2>&1 | tee "$EV/phase8b-endpoints.log"

echo "== graceful shutdown (SIGTERM) =="
kill -TERM $APP_PID
exited=0
for i in $(seq 1 40); do kill -0 $APP_PID 2>/dev/null || { exited=$i; break; }; sleep 1; done
[ "$exited" != 0 ] || { echo "APP DID NOT EXIT AFTER SIGTERM (40s)"; kill -9 $APP_PID; exit 1; }
echo "app exited ${exited}s after SIGTERM"
grep -iE 'shutdown|graceful|stopped|exiting|signal' "$EV/phase8b-app.log" | tail -8 | tee "$EV/phase8b-shutdown.log"
echo "== phase8b DONE $(date -u +%Y-%m-%dT%H:%M:%SZ) =="
