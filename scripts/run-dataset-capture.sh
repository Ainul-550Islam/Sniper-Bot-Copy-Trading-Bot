#!/usr/bin/env bash
# BATCH 02 — recorded launch-data capture runbook (LIVE FEED REQUIRED).
#
# This script drives a BOUNDED (>= 10 minute) capture session and then seals +
# verifies the resulting dataset. It is a TEMPLATE for a real capture: it will
# NEVER create a synthetic_fixture dataset and will NEVER fabricate data. A
# synthetic fixture import (`dataset_tool import-fixtures`) is a completely
# different, clearly-labelled operation and is intentionally NOT used here.
#
# The live wiring (Sniper::with_recorder + the server spawning Recorder::run)
# is attached by the integration batch. Until then this script FAILS CLOSED
# unless the operator explicitly supplies a real capture command via
# DATASET_CAPTURE_CMD and confirms the wiring with DATASET_CAPTURE_WIRED=1.
#
# Usage (operator, on a capture host with real feeds configured):
#   DATASET_DIR=/var/lib/sniper/datasets \
#   DATASET_CAPTURE_WIRED=1 \
#   DATASET_CAPTURE_CMD='cargo run -q --release -p sniper-suite --bin sniper-suite' \
#   ./scripts/run-dataset-capture.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# --- Validate the environment (fail-closed; no fabricated defaults) ---------
: "${DATASET_DIR:?set DATASET_DIR to the dataset output root}"
DURATION="${DATASET_CAPTURE_SECONDS:-600}"
if ! [[ "$DURATION" =~ ^[0-9]+$ ]] || (( DURATION < 600 )); then
  echo "DATASET_CAPTURE_SECONDS must be an integer >= 600 (bounded >= 10 min session)" >&2
  exit 2
fi
export DATASET_CHUNK_MAX_RECORDS="${DATASET_CHUNK_MAX_RECORDS:-1000}"
export DATASET_CHUNK_MAX_BYTES="${DATASET_CHUNK_MAX_BYTES:-4194304}"
export DATASET_CHANNEL_CAPACITY="${DATASET_CHANNEL_CAPACITY:-4096}"
export DATASET_MIN_RECORDS="${DATASET_MIN_RECORDS:-1}"
export DATASET_MIN_SLOT_SPAN="${DATASET_MIN_SLOT_SPAN:-0}"
export DATASET_MAX_SLOT_GAP="${DATASET_MAX_SLOT_GAP:-10000}"

if [[ "${DATASET_CAPTURE_WIRED:-0}" != "1" ]]; then
  echo "REFUSING: live capture is not wired. Batch 02 supplies the recorder;" >&2
  echo "the integration batch must attach Sniper::with_recorder and spawn" >&2
  echo "Recorder::run before a real feed can be recorded. No data was created." >&2
  exit 2
fi
: "${DATASET_CAPTURE_CMD:?set DATASET_CAPTURE_CMD to the real capture command}"
: "${SOLANA_RPC_URL:?a real RPC endpoint is required; never capture from fixtures}"

mkdir -p "$DATASET_DIR"
echo "capture: starting bounded ${DURATION}s session into ${DATASET_DIR}"

# --- Bounded capture -------------------------------------------------------
# SIGTERM at the deadline triggers a graceful shutdown, which seals the
# dataset (Recorder::run seals on shutdown). SIGKILL is a last resort.
set +e
timeout --signal=TERM --kill-after=30s "${DURATION}s" bash -c "$DATASET_CAPTURE_CMD"
rc=$?
set -e
if (( rc != 0 && rc != 124 && rc != 143 )); then
  echo "capture: capture command exited with ${rc}" >&2
  exit "$rc"
fi
echo "capture: session ended (rc=${rc}); the recorder should now be sealed"

# --- Locate the newest dataset and verify it -------------------------------
LATEST=""
if [[ -f "$DATASET_DIR/LATEST" ]]; then
  LATEST="$(tr -d '[:space:]' < "$DATASET_DIR/LATEST")"
fi
if [[ -z "$LATEST" ]]; then
  for d in "$DATASET_DIR"/*/; do
    if [[ -f "${d}manifest.json" ]]; then
      LATEST="$(basename "$d")"
      break
    fi
  done
fi
if [[ -z "$LATEST" || ! -f "$DATASET_DIR/$LATEST/manifest.json" ]]; then
  echo "capture: no dataset manifest found — nothing was captured (no data invented)" >&2
  exit 1
fi

echo "capture: verifying ${DATASET_DIR}/${LATEST}"
cargo run -q -p module-sniper --bin dataset_tool -- verify "$DATASET_DIR/$LATEST"
echo "capture: replay hash:"
cargo run -q -p module-sniper --bin dataset_tool -- replay-hash "$DATASET_DIR/$LATEST"
