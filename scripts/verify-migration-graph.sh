#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS="$ROOT/crates/core/migrations"

mapfile -t files < <(find "$MIGRATIONS" -maxdepth 1 -type f -name '*.sql' -printf '%f\n' | sort)
if ((${#files[@]} == 0)); then
  echo "verify-migration-graph: no migrations found" >&2
  exit 1
fi

for index in "${!files[@]}"; do
  expected=$(printf '%04d' "$((index + 1))")
  prefix="${files[$index]:0:4}"
  if [[ "$prefix" != "$expected" ]]; then
    echo "verify-migration-graph: expected migration $expected, found ${files[$index]}" >&2
    exit 1
  fi
done

# Migration 0001 owns the deployment-level operator strategy table. The
# tenant strategy table introduced by 0037 must have a distinct name, or a
# fresh database fails when 0037's organization_id indexes are applied.
operator_creators=$(grep -RIl --include='*.sql' -E '^CREATE TABLE IF NOT EXISTS strategies[[:space:]]*\(' "$MIGRATIONS" | wc -l)
if [[ "$operator_creators" != "1" ]] || ! grep -qE '^CREATE TABLE IF NOT EXISTS strategies[[:space:]]*\(' "$MIGRATIONS/0001_bootstrap.sql"; then
  echo "verify-migration-graph: deployment-level strategies table must be declared exactly once in 0001" >&2
  exit 1
fi

if ! grep -qE '^CREATE TABLE IF NOT EXISTS tenant_strategies[[:space:]]*\(' "$MIGRATIONS/0037_commercial_strategy_backtest_webhooks.sql"; then
  echo "verify-migration-graph: 0037 must create tenant_strategies" >&2
  exit 1
fi
if ! grep -q 'REFERENCES tenant_strategies(id)' "$MIGRATIONS/0037_commercial_strategy_backtest_webhooks.sql"; then
  echo "verify-migration-graph: backtest_runs must reference tenant_strategies" >&2
  exit 1
fi
if grep -qE 'ON strategies[[:space:]]*\(' "$MIGRATIONS/0037_commercial_strategy_backtest_webhooks.sql"; then
  echo "verify-migration-graph: 0037 still indexes deployment-level strategies" >&2
  exit 1
fi

# Tenant strategy SQL must not silently return to the deployment-level table.
if grep -RInE '(FROM|INTO|UPDATE|JOIN)[[:space:]]+strategies([[:space:]\n]|$)' \
    "$ROOT/crates/server/src/trading_data_plane" --include='*.rs' >/dev/null; then
  echo "verify-migration-graph: trading data plane targets deployment-level strategies" >&2
  exit 1
fi

echo "verify-migration-graph: ${#files[@]} contiguous migrations; operator and tenant strategy tables are separated"
