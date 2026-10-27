#!/usr/bin/env bash
# BATCH 02 ownership guard.
#
# Asserts that every file Batch 02 adds/modifies lies within the Batch 02
# target tree (plus the three deliverable meta-files). This prevents
# accidental drift into Batch 01 (0060-0062, tenant/**, saas/**, openapi,
# control-plane UI, ci.yml, Cargo.lock, ...) or any other parallel batch's
# files. Run from anywhere; paths are always repo-relative.
#
# TWO MODES — the file set is chosen by tree shape, never by a flag:
#
#  * ISOLATED batch working tree (HEAD is at BASE_COMMIT.txt, changes are
#    uncommitted): strict `git diff BASE` + untracked files. Every changed
#    path must be allowed. This is the drift guard used while developing the
#    batch and while staging it with APPLY.sh.
#
#  * MERGED / CI checkout (Batch 01 + Batch 02 already committed, or a
#    shallow actions/checkout where BASE_COMMIT.txt's commit is not even in
#    the clone): the base diff legitimately contains OTHER batches' files,
#    which Batch 02 does not own. Claiming them would make datasets.yml fail
#    on its own target tree. There the guard scopes itself to Batch 02's
#    authoritative shipped manifest, SHA256SUMS (every path in it, plus
#    SHA256SUMS itself), and additionally checks that each path exists and
#    that the manifest was not stripped of Batch 02's essential files.
#    Mode is chosen by COMMIT HISTORY (HEAD past BASE), not by worktree
#    cleanliness: a merged checkout with local edits must not be pushed into
#    strict base-diff mode, where other batches' files would false-positive.
#
# If neither a usable base nor SHA256SUMS is available, the guard compares
# against HEAD, which is empty on a clean checkout (trivially satisfied).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MANIFEST="$ROOT/SHA256SUMS"
BASE_FILE="$ROOT/BASE_COMMIT.txt"

BASE=""
if [[ -f "$BASE_FILE" ]]; then
  BASE="$(tr -d '[:space:]' < "$BASE_FILE")"
fi

have_base=0
if [[ -n "$BASE" ]] && git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
  have_base=1
fi

# Decide whether we are on the merged/CI tree. A shallow CI clone cannot
# resolve BASE at all; a merged clone has HEAD past BASE with BASE as an
# ancestor. In both cases SHA256SUMS is the only reliable statement of
# Batch 02's own footprint. Worktree cleanliness is deliberately NOT part of
# the decision: uncommitted edits in a merged checkout are not Batch 02's
# committed footprint and must not silently switch us back to strict mode.
use_manifest=0
if [[ -f "$MANIFEST" ]] && (( ! have_base )); then
  use_manifest=1
elif [[ -f "$MANIFEST" ]] && (( have_base )); then
  if [[ "$(git rev-parse HEAD)" != "$(git rev-parse "${BASE}^{commit}")" ]] \
     && git merge-base --is-ancestor "$BASE" HEAD 2>/dev/null; then
    use_manifest=1
  fi
fi

if (( use_manifest )); then
  echo "check-batch02-ownership: merged/CI checkout -> guarding SHA256SUMS manifest"
  # sha256sum lines are "<64 hex><two spaces or space-star><path>".
  changed="$( { sed -E 's/^[0-9a-fA-F]{64}[[:space:]*]+//' "$MANIFEST"
                echo SHA256SUMS; } \
             | sed '/^[[:space:]]*$/d' | sort -u )"
else
  if (( ! have_base )); then
    BASE="$(git rev-parse HEAD)"
  fi
  # Tracked modifications (working tree vs base) UNION untracked new files.
  changed="$( { git diff --name-only "$BASE" --; git ls-files --others --exclude-standard; } \
             | sed '/^[[:space:]]*$/d' | sort -u )"
fi

# The ONLY paths Batch 02 may add or modify.
is_allowed() {
  case "$1" in
    # Deliverable meta-files.
    BASE_COMMIT.txt|APPLY.sh|SHA256SUMS) return 0 ;;
    # Batch 02 scripts / CI / docs.
    scripts/run-dataset-capture.sh|scripts/check-batch02-ownership.sh) return 0 ;;
    .github/workflows/datasets.yml) return 0 ;;
    docs/BACKTEST-DATASETS.md|docs/gap-updates/batch-02-rows.md) return 0 ;;
    # Core migrations 0063-0065 (NOT 0060-0062 — those are Batch 01).
    crates/core/migrations/0063_*.sql|crates/core/migrations/0064_*.sql|crates/core/migrations/0065_*.sql) return 0 ;;
    # module-sniper: manifest, lib hook, dataset module, bin, integration tests.
    crates/module-sniper/Cargo.toml|crates/module-sniper/src/lib.rs) return 0 ;;
    crates/module-sniper/src/dataset/*.rs) return 0 ;;
    crates/module-sniper/src/bin/dataset_tool.rs) return 0 ;;
    crates/module-sniper/tests/dataset_*.rs) return 0 ;;
    # server: trading data plane store + one-line module declaration + tests.
    crates/server/src/trading_data_plane/mod.rs|crates/server/src/trading_data_plane/dataset_store_pg.rs) return 0 ;;
    crates/server/tests/dataset_pg_store.rs|crates/server/tests/dataset_migrations.rs) return 0 ;;
    *) return 1 ;;
  esac
}

bad=0
count=0
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  count=$((count + 1))
  if ! is_allowed "$f"; then
    echo "check-batch02-ownership: FORBIDDEN changed path: $f" >&2
    bad=1
  fi
  # In manifest mode the manifest is the assertion itself: a stale or
  # hand-edited path must not pass just because it looks allowed.
  if (( use_manifest )) && [[ ! -e "$ROOT/$f" ]]; then
    echo "check-batch02-ownership: manifest path missing on disk: $f" >&2
    bad=1
  fi
done <<< "$changed"

# In manifest mode the manifest is the guard's input, so a stripped or
# hand-edited manifest must not be able to pass by simply being short. These
# five paths are the irreducible core of Batch 02 and are never optional.
if (( use_manifest )); then
  for sentinel in \
    crates/core/migrations/0063_launch_datasets.sql \
    crates/module-sniper/src/dataset/verifier.rs \
    crates/module-sniper/src/bin/dataset_tool.rs \
    crates/server/src/trading_data_plane/dataset_store_pg.rs \
    scripts/run-dataset-capture.sh; do
    if ! grep -qxF "$sentinel" <<< "$changed"; then
      echo "check-batch02-ownership: manifest missing essential Batch 02 file: $sentinel" >&2
      bad=1
    fi
  done
fi

if (( bad )); then
  echo "check-batch02-ownership: FAIL — paths outside Batch 02 ownership were changed" >&2
  exit 1
fi
echo "check-batch02-ownership: OK — ${count} changed file(s), all within Batch 02 ownership"
