#!/usr/bin/env bash
# BATCH 02 — opt-in capture from the real configured Solana feeds.
#
# No live command runs unless LIVE_DATASET_CAPTURE=1. A real capture also
# requires the integration batch's recorder wiring, both RPC/WS endpoints,
# and an explicit operator command. Synthetic fixture import is never used by
# this script. Evidence is written only below evidence/local/; the immutable
# evidence/live/ records are never edited here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "${LIVE_DATASET_CAPTURE:-0}" != "1" ]]; then
    echo "NOT_RUN"
    exit 0
fi

EVIDENCE_DIR="$ROOT/evidence/local"
EVIDENCE_FILE="$EVIDENCE_DIR/launch_dataset_capture.json"
mkdir -p "$EVIDENCE_DIR"

DS_DATASET_ID=""
DS_SOURCE=""
DS_AUTHORITATIVE="false"
DS_HASH_CHAIN_OK="false"
DS_COVERAGE_OK="false"
DS_OUTCOME=""
DS_RECORD_COUNT=""
DS_SHA256_ROOT=""
DS_REPLAY_HASH=""
DS_CAPTURE_RC=""
DS_VERIFY_OUTPUT=""

write_evidence() {
    local status="$1"
    local reason="$2"
    local tmp="$EVIDENCE_FILE.tmp.$$"
    local dataset_id_json=""
    local source_json=""
    local outcome_json=""
    local sha256_root_json=""
    local replay_hash_json=""
    local authoritative_json="false"
    local hash_chain_ok_json="false"
    local coverage_ok_json="false"
    local record_count_json="null"
    local capture_rc_json="null"

    # Evidence must remain valid JSON even when the failure being reported is
    # malformed tool output or an invalid dataset directory name.
    if [[ "$DS_DATASET_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ \
        && "$DS_DATASET_ID" != "." && "$DS_DATASET_ID" != ".." ]]; then
        dataset_id_json="$DS_DATASET_ID"
    fi
    if [[ "$DS_SOURCE" == "recorded" || "$DS_SOURCE" == "synthetic_fixture" ]]; then
        source_json="$DS_SOURCE"
    fi
    if [[ "$DS_OUTCOME" == "pass" || "$DS_OUTCOME" == "fail" ]]; then
        outcome_json="$DS_OUTCOME"
    fi
    if [[ "$DS_SHA256_ROOT" =~ ^[0-9a-f]{64}$ ]]; then
        sha256_root_json="$DS_SHA256_ROOT"
    fi
    if [[ "$DS_REPLAY_HASH" =~ ^[0-9a-f]{64}$ ]]; then
        replay_hash_json="$DS_REPLAY_HASH"
    fi
    [[ "$DS_AUTHORITATIVE" == "true" ]] && authoritative_json="true"
    [[ "$DS_HASH_CHAIN_OK" == "true" ]] && hash_chain_ok_json="true"
    [[ "$DS_COVERAGE_OK" == "true" ]] && coverage_ok_json="true"
    [[ "$DS_RECORD_COUNT" =~ ^[0-9]+$ ]] && record_count_json="$DS_RECORD_COUNT"
    [[ "$DS_CAPTURE_RC" =~ ^[0-9]+$ ]] && capture_rc_json="$DS_CAPTURE_RC"

    cat > "$tmp" <<EOF
{
  "validation_id": "launch_dataset_capture",
  "environment": "local_live_feed_capture",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "status": "$status",
  "reason": "$reason",
  "dataset_id": "$dataset_id_json",
  "source": "$source_json",
  "authoritative": $authoritative_json,
  "hash_chain_ok": $hash_chain_ok_json,
  "coverage_ok": $coverage_ok_json,
  "outcome": "$outcome_json",
  "record_count": $record_count_json,
  "sha256_root": "$sha256_root_json",
  "replay_hash": "$replay_hash_json",
  "capture_exit_code": $capture_rc_json
}
EOF
    mv "$tmp" "$EVIDENCE_FILE"
}

fail_capture() {
    local reason="$1"
    local rc="${2:-1}"
    echo "capture: FAILED: $reason" >&2
    write_evidence "FAILED" "$reason"
    exit "$rc"
}

require_env() {
    local name="$1"
    if [[ -z "${!name:-}" ]]; then
        fail_capture "missing_required_environment_$name" 2
    fi
}

require_env DATASET_DIR
require_env SOLANA_RPC_URL
require_env SOLANA_WS_URL
require_env DATASET_CAPTURE_CMD

if [[ "${DATASET_CAPTURE_WIRED:-0}" != "1" ]]; then
    fail_capture "live_recorder_wiring_is_not_confirmed" 2
fi
if ! command -v cargo >/dev/null 2>&1; then
    fail_capture "cargo_is_not_available" 2
fi
if ! command -v timeout >/dev/null 2>&1; then
    fail_capture "timeout_is_not_available" 2
fi

DURATION="${DATASET_CAPTURE_SECONDS:-600}"
if ! [[ "$DURATION" =~ ^[0-9]+$ ]] || (( DURATION < 600 )); then
    fail_capture "DATASET_CAPTURE_SECONDS_must_be_an_integer_at_least_600" 2
fi

export DATASET_CHUNK_MAX_RECORDS="${DATASET_CHUNK_MAX_RECORDS:-1000}"
export DATASET_CHUNK_MAX_BYTES="${DATASET_CHUNK_MAX_BYTES:-4194304}"
export DATASET_CHANNEL_CAPACITY="${DATASET_CHANNEL_CAPACITY:-4096}"
export DATASET_MIN_RECORDS="${DATASET_MIN_RECORDS:-1}"
export DATASET_MIN_SLOT_SPAN="${DATASET_MIN_SLOT_SPAN:-0}"
export DATASET_MAX_SLOT_GAP="${DATASET_MAX_SLOT_GAP:-10000}"

mkdir -p "$DATASET_DIR"
echo "capture: starting bounded ${DURATION}s real-feed session"

# SIGTERM at the deadline lets the integration owner gracefully seal the
# recorder. SIGKILL is only the last resort after the graceful grace period.
set +e
timeout --signal=TERM --kill-after=30s "${DURATION}s" bash -c "$DATASET_CAPTURE_CMD"
DS_CAPTURE_RC=$?
set -e
if (( DS_CAPTURE_RC != 0 && DS_CAPTURE_RC != 124 && DS_CAPTURE_RC != 143 )); then
    fail_capture "capture_command_failed" "$DS_CAPTURE_RC"
fi
echo "capture: session ended (exit=$DS_CAPTURE_RC); locating sealed dataset"

if [[ -f "$DATASET_DIR/LATEST" ]]; then
    DS_DATASET_ID="$(tr -d '[:space:]' < "$DATASET_DIR/LATEST")"
fi
if [[ -n "$DS_DATASET_ID" ]] \
    && { [[ "$DS_DATASET_ID" == "." || "$DS_DATASET_ID" == ".." ]] \
        || ! [[ "$DS_DATASET_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; }; then
    fail_capture "invalid_dataset_id_in_LATEST" 1
fi

# LATEST is optional for the live integration. Without it, select the newest
# manifest rather than an arbitrary directory or a fabricated dataset.
if [[ -z "$DS_DATASET_ID" || ! -f "$DATASET_DIR/$DS_DATASET_ID/manifest.json" ]]; then
    DS_DATASET_ID=""
    newest_manifest=""
    for manifest in "$DATASET_DIR"/*/manifest.json; do
        [[ -f "$manifest" ]] || continue
        if [[ -z "$newest_manifest" || "$manifest" -nt "$newest_manifest" ]]; then
            newest_manifest="$manifest"
        fi
    done
    if [[ -n "$newest_manifest" ]]; then
        DS_DATASET_ID="$(basename "$(dirname "$newest_manifest")")"
    fi
fi
if [[ -z "$DS_DATASET_ID" || ! -f "$DATASET_DIR/$DS_DATASET_ID/manifest.json" ]]; then
    fail_capture "no_dataset_manifest_was_captured" 1
fi

DATASET_PATH="$DATASET_DIR/$DS_DATASET_ID"
echo "capture: verifying $DATASET_PATH"
set +e
DS_VERIFY_OUTPUT="$(cargo run --locked -q -p module-sniper --bin dataset_tool -- verify "$DATASET_PATH" 2>&1)"
verify_rc=$?
set -e
printf '%s\n' "$DS_VERIFY_OUTPUT"

verify_field() {
    local key="$1"
    sed -n "s/^${key}=//p" <<< "$DS_VERIFY_OUTPUT" | tail -n 1
}

DS_SOURCE="$(verify_field source)"
DS_AUTHORITATIVE="$(verify_field authoritative)"
DS_HASH_CHAIN_OK="$(verify_field hash_chain_ok)"
DS_COVERAGE_OK="$(verify_field coverage_ok)"
DS_OUTCOME="$(verify_field outcome)"
DS_RECORD_COUNT="$(verify_field record_count)"
DS_SHA256_ROOT="$(verify_field sha256_root)"
DS_AUTHORITATIVE="${DS_AUTHORITATIVE:-false}"
DS_HASH_CHAIN_OK="${DS_HASH_CHAIN_OK:-false}"
DS_COVERAGE_OK="${DS_COVERAGE_OK:-false}"

if (( verify_rc != 0 )); then
    fail_capture "dataset_verification_failed" "$verify_rc"
fi
if [[ "$DS_SOURCE" != "recorded" \
    || "$DS_OUTCOME" != "pass" \
    || "$DS_HASH_CHAIN_OK" != "true" \
    || "$DS_COVERAGE_OK" != "true" \
    || "$DS_AUTHORITATIVE" != "true" ]]; then
    fail_capture "verified_dataset_is_not_authoritative_recorded_data" 1
fi
if ! [[ "$DS_RECORD_COUNT" =~ ^[0-9]+$ ]] \
    || ! [[ "$DS_SHA256_ROOT" =~ ^[0-9a-f]{64}$ ]]; then
    fail_capture "verifier_output_is_incomplete" 1
fi

set +e
replay_output="$(cargo run --locked -q -p module-sniper --bin dataset_tool -- replay-hash "$DATASET_PATH" 2>&1)"
replay_rc=$?
set -e
printf '%s\n' "$replay_output"
if (( replay_rc != 0 )); then
    fail_capture "deterministic_replay_hash_failed" "$replay_rc"
fi
DS_REPLAY_HASH="$(tail -n 1 <<< "$replay_output")"
if ! [[ "$DS_REPLAY_HASH" =~ ^[0-9a-f]{64}$ ]]; then
    DS_REPLAY_HASH=""
    fail_capture "replay_hash_output_is_invalid" 1
fi

write_evidence "PASSED" "real_feed_capture_and_verification_passed"
echo "capture: PASSED evidence=$EVIDENCE_FILE"
