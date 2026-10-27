#!/usr/bin/env bash
# Fail if this batch changed any path outside its parallel-ownership contract.
# The diff always starts at the exact commit recorded when Batch 02 began;
# untracked files are included so a forgotten new file cannot evade the gate.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ ! -f BASE_COMMIT.txt ]]; then
    echo "check-batch02-ownership: BASE_COMMIT.txt is missing" >&2
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if ! [[ "$BASE" =~ ^[0-9a-f]{40}$ ]] \
    || ! git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
    echo "check-batch02-ownership: invalid or unavailable base commit: $BASE" >&2
    exit 1
fi

is_allowed() {
    case "$1" in
        BASE_COMMIT.txt|APPLY.sh|SHA256SUMS) return 0 ;;
        .github/workflows/datasets.yml) return 0 ;;
        docs/BACKTEST-DATASETS.md|docs/gap-updates/batch-02-rows.md) return 0 ;;
        scripts/run-dataset-capture.sh|scripts/check-batch02-ownership.sh) return 0 ;;
        crates/core/migrations/0063_*.sql|crates/core/migrations/0064_*.sql|crates/core/migrations/0065_*.sql) return 0 ;;
        crates/module-sniper/Cargo.toml|crates/module-sniper/src/lib.rs) return 0 ;;
        crates/module-sniper/src/dataset/*.rs) return 0 ;;
        crates/module-sniper/src/bin/dataset_tool.rs) return 0 ;;
        crates/module-sniper/tests/dataset_*.rs) return 0 ;;
        crates/server/src/trading_data_plane/mod.rs|crates/server/src/trading_data_plane/dataset_store_pg.rs) return 0 ;;
        crates/server/tests/dataset_pg_store.rs|crates/server/tests/dataset_migrations.rs) return 0 ;;
        *) return 1 ;;
    esac
}

mapfile -t changed < <(
    {
        git diff --name-only "$BASE" --
        git ls-files --others --exclude-standard
    } | sed '/^[[:space:]]*$/d' | sort -u
)

bad=0
for path in "${changed[@]}"; do
    if ! is_allowed "$path"; then
        echo "check-batch02-ownership: FORBIDDEN changed path: $path" >&2
        bad=1
    fi
done
if (( bad )); then
    echo "check-batch02-ownership: FAIL" >&2
    exit 1
fi

echo "check-batch02-ownership: PASS — ${#changed[@]} changed file(s), all owned by Batch 02"
