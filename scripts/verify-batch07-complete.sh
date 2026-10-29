#!/usr/bin/env bash
# verify-batch07-complete.sh — completeness gate for the Batch 07 tree.
#
# FILES-MANIFEST.txt describes the combined delivered tree (integration
# cleanup + Batch 07) as lines of:
#
#   <repo-relative path>\t<newline count>\t<sha256>
#
# This verifier fails when a manifest path is unsafe, duplicated, missing,
# empty, below its category minimum, or different from the recorded line
# count/hash. It then runs the Batch 07 ownership gate, scans the Batch 07
# code surface for placeholder-style forbidden patterns, parses the
# generated JSON artifacts, and syntax-checks the new shell entry points.
#
# It does not replace the full Cargo/PostgreSQL/frontend acceptance run;
# those commands and their results are recorded in the batch report. What
# this proves is mechanical: the delivered tree is complete, internally
# consistent, and inside its recorded ownership.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MANIFEST="FILES-MANIFEST.txt"
EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
fail=0

note() { echo "[verify-batch07] $*"; }
error() { echo "[verify-batch07] FAIL — $*" >&2; fail=1; }

if [[ ! -f "$MANIFEST" ]]; then
    error "$MANIFEST is missing"
    exit 1
fi
if [[ ! -f BASE_COMMIT.txt ]]; then
    error "BASE_COMMIT.txt is missing"
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if [[ "$BASE" != "$EXPECTED_BASE" ]]; then
    error "BASE_COMMIT.txt contains $BASE, expected $EXPECTED_BASE"
fi

# Generated artifacts and pre-existing short wrappers have no meaningful
# generic line minimum; their hashes still have to match exactly.
is_generated_or_wrapper() {
    case "$1" in
        openapi/openapi.json) return 0 ;;
        release-manifest.json) return 0 ;;
        crates/module-sniper/fixtures/replay/*.json) return 0 ;;
        scripts/untrack-batch01-junk.sh) return 0 ;;
        *) return 1 ;;
    esac
}

minimum_lines_for() {
    local path="$1"
    if is_generated_or_wrapper "$path"; then
        echo 1
        return
    fi
    case "$path" in
        crates/server/src/telegram_trade_port.rs) echo 800 ;;
        scripts/check-batch07-ownership.sh) echo 100 ;;
        scripts/verify-batch07-complete.sh) echo 100 ;;
        scripts/check-cleanup-ownership.sh) echo 250 ;;
        scripts/verify-batch-complete.sh) echo 250 ;;
        APPLY.sh) echo 150 ;;
        .github/workflows/*.yml) echo 100 ;;
        openapi/openapi.yaml) echo 100 ;;
        docs/STATS.md) echo 20 ;;
        *.rs|*.ts|*.tsx|*.sh) echo 40 ;;
        *.md) echo 20 ;;
        *) echo 1 ;;
    esac
}

declare -A seen_paths=()
checked=0
while IFS=$'\t' read -r path expected_lines expected_sha; do
    [[ -z "$path" ]] && continue
    case "$path" in
        /*|*..*|*\\*)
            error "unsafe manifest path: $path"
            continue
            ;;
    esac
    if [[ -n "${seen_paths[$path]:-}" ]]; then
        error "duplicate manifest path: $path"
        continue
    fi
    seen_paths["$path"]=1
    if [[ ! -f "$path" ]]; then
        error "manifest path missing on disk: $path"
        continue
    fi
    if [[ ! -s "$path" ]]; then
        error "manifest path is empty: $path"
        continue
    fi
    actual_lines="$(python3 -c 'import sys; data=open(sys.argv[1],"rb").read(); print(data.count(bytes([10])))' "$path")"
    if [[ "$actual_lines" != "$expected_lines" ]]; then
        error "line count mismatch for $path: manifest $expected_lines, disk $actual_lines"
    fi
    minimum="$(minimum_lines_for "$path")"
    if [[ "$actual_lines" -lt "$minimum" ]]; then
        error "file below category minimum ($actual_lines < $minimum lines): $path"
    fi
    actual_sha="$(sha256sum "$path" | cut -d' ' -f1)"
    if [[ "$actual_sha" != "$expected_sha" ]]; then
        error "sha256 mismatch for $path"
    fi
    checked=$((checked + 1))
done < "$MANIFEST"
note "manifest entries verified: $checked"

# Generated JSON must parse (they are contracts, not text blobs).
for json in openapi/openapi.json release-manifest.json; do
    if ! python3 -c 'import json,sys; json.load(open(sys.argv[1], encoding="utf-8"))' "$json"; then
        error "invalid JSON: $json"
    fi
done

# New shell entry points must at least parse.
for shell in scripts/check-batch07-ownership.sh scripts/verify-batch07-complete.sh; do
    if ! bash -n "$shell"; then
        error "shell syntax error: $shell"
    fi
done

# Forbidden placeholder patterns on the Batch 07 code surface. Test-only
# unwrap/expect are not scanned for here (they are reviewed separately);
# this gate is about unfinished-work markers and disabled tests.
BATCH07_CODE=(
    crates/core/src/config.rs
    crates/core/src/models.rs
    crates/core/src/tenant/rls_context.rs
    crates/module-copy/src/feeds.rs
    crates/module-copy/src/policy.rs
    crates/module-sniper/src/detect.rs
    crates/module-sniper/src/entry.rs
    crates/module-sniper/src/event.rs
    crates/module-sniper/src/exit.rs
    crates/module-sniper/src/market.rs
    crates/module-sniper/src/pipeline.rs
    crates/module-telegram/src/lib.rs
    crates/server/src/main.rs
    crates/server/src/telegram_trade_port.rs
    crates/server/src/trading_data_plane/telegram.rs
    crates/solana-kit/src/decode.rs
    crates/solana-kit/src/venues/raydium_launchlab/decode.rs
    apps/control-plane/src/app/trading/telegram/page.tsx
    apps/control-plane/src/lib/customer-trading-api.ts
)
pattern='todo!\(\)|unimplemented!\(\)|\bTODO\b|\bFIXME\b|#\[ignore\]|\|\| true'
for file in "${BATCH07_CODE[@]}"; do
    # Only lines this batch added are judged: pre-existing text in these
    # files belongs to earlier batches (e.g. `args().skip(1)`), while
    # anything Batch 07 introduced must be finished work.
    added="$(git diff "$BASE" -- "$file" | awk '/^\+/ && !/^\+\+\+/')"
    if grep -E "$pattern" <<<"$added" >/dev/null; then
        error "forbidden pattern added in $file:"
        grep -E "$pattern" <<<"$added" >&2
    fi
done

# Ownership: nothing outside the recorded target tree may have changed.
if ! bash scripts/check-batch07-ownership.sh; then
    error "ownership gate failed"
fi

if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
note "PASS"
