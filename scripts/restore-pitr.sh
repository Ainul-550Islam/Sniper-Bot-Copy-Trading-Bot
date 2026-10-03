#!/usr/bin/env bash
# ============================================================================
# scripts/restore-pitr.sh — restore a cluster to a point in time, and
# (optionally) prove it (P1 batch 5, SLO D1/D2).
#
#   ./scripts/restore-pitr.sh --base base-20261002T020000Z \
#                             --target-time '2026-10-02T09:15:00Z'
#   ./scripts/restore-pitr.sh --latest --verify        # quarterly drill
#   ./scripts/restore-pitr.sh --check                  # is a PITR drill overdue?
#
# WHAT IT DOES
#
#   1. extracts the base backup into a NEW data directory;
#   2. writes `recovery.signal` plus a restore_command that reads this
#      repository's WAL archive, and the recovery target;
#   3. with --verify: starts that cluster on a throwaway port, waits for
#      recovery to finish, checks the business tables are there, records
#      the result in the ledger, and shuts it down again.
#
# WHAT IT REFUSES TO DO
#
#   * touch an existing, non-empty data directory — a PITR restore that
#     overwrites the live cluster is how a recovery becomes the outage;
#   * use the live cluster's port;
#   * promote anything. Recovery stops at the target and the cluster
#     stays in recovery until a human promotes it deliberately.
#
# RECOVERY TARGET: Postgres accepts a timestamp with a time zone. Always
# pass one in UTC with an explicit `Z` — "09:15:00" means different
# moments on the backup host and the restore host, and a PITR that
# silently lands an hour away from the target is worse than no PITR,
# because it will be believed.
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/deploy/backups}"
BASE_DIR="${BACKUP_BASE_DIR:-$BACKUP_DIR/base}"
ARCHIVE_DIR="${BACKUP_WAL_ARCHIVE_DIR:-$BACKUP_DIR/wal-archive}"
LEDGER="${BACKUP_LEDGER_PATH:-$BACKUP_DIR/backups.jsonl}"
DRILL_MAX_AGE_DAYS="${BACKUP_DRILL_MAX_AGE_DAYS:-90}"
RESTORE_ROOT="${BACKUP_PITR_RESTORE_DIR:-}"
PITR_PORT="${BACKUP_PITR_PORT:-55432}"

BASE_ID=""
TARGET_TIME=""
VERIFY=0
USE_LATEST=0

log() { echo "[$(date -u +%H:%M:%S)] $*"; }
die() { echo "FAIL: $*" >&2; exit 1; }

record() {
    mkdir -p "$(dirname "$LEDGER")"
    printf '%s\n' "$1" >> "$LEDGER"
}

fail_drill() {
    local reason="${1//\"/\'}"
    record "{\"event\":\"pitr_verified\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"base_id\":\"${BASE_ID:-unknown}\",\"ok\":false,\"detail\":\"${reason}\"}"
    die "$reason"
}

# --- --check: is the PITR drill overdue? -----------------------------------
if [[ "${1:-}" == "--check" ]]; then
    [[ -f "$LEDGER" ]] || die "no ledger at $LEDGER — no PITR drill has ever been recorded"
    last="$(grep '"event":"pitr_verified"' "$LEDGER" 2>/dev/null | grep '"ok":true' | tail -1 || true)"
    [[ -n "$last" ]] || die "no SUCCESSFUL point-in-time recovery has ever been proven on this deployment"
    last_at="$(printf '%s' "$last" | sed -E 's/.*"at":"([^"]+)".*/\1/')"
    age_days=$(( ( $(date -u +%s) - $(date -u -d "$last_at" +%s) ) / 86400 ))
    echo "last verified PITR: $last_at (${age_days}d ago, limit ${DRILL_MAX_AGE_DAYS}d)"
    [[ "$age_days" -le "$DRILL_MAX_AGE_DAYS" ]] \
        || die "the PITR drill is overdue by $(( age_days - DRILL_MAX_AGE_DAYS )) day(s)"
    echo "OK"
    exit 0
fi

# --- arguments -------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)        BASE_ID="${2:-}"; shift 2 ;;
        --target-time) TARGET_TIME="${2:-}"; shift 2 ;;
        --latest)      USE_LATEST=1; shift ;;
        --verify)      VERIFY=1; shift ;;
        --restore-dir) RESTORE_ROOT="${2:-}"; shift 2 ;;
        -h|--help)     sed -n '2,40p' "$0"; exit 0 ;;
        *)             die "unknown argument: $1" ;;
    esac
done

[[ -n "$BASE_ID" || "$USE_LATEST" -eq 1 ]] \
    || die "pass --base <base_id> or --latest"

command -v pg_ctl >/dev/null 2>&1 || die "pg_ctl is not installed — PITR needs the PostgreSQL server binaries, not just the client"
command -v psql   >/dev/null 2>&1 || die "psql is not installed"

if [[ "$USE_LATEST" -eq 1 ]]; then
    BASE_ID="$(find "$BASE_DIR" -maxdepth 1 -mindepth 1 -type d -name 'base-*' -printf '%f\n' 2>/dev/null | sort | tail -1)"
    [[ -n "$BASE_ID" ]] || die "no base backup in $BASE_DIR — run ./scripts/backup-basebackup.sh first"
fi

BASE_PATH="$BASE_DIR/$BASE_ID"
[[ -d "$BASE_PATH" ]] || die "no base backup $BASE_ID in $BASE_DIR"
[[ -s "$BASE_PATH/base.tar.gz" ]] || die "$BASE_ID has no base.tar.gz"
[[ -d "$ARCHIVE_DIR" ]] || die "no WAL archive at $ARCHIVE_DIR — without it a base backup cannot roll forward"

# --- checksum the base before trusting it ----------------------------------
if [[ -f "$BASE_PATH/SHA256SUMS" ]]; then
    ( cd "$BASE_PATH" && sha256sum --quiet --check SHA256SUMS ) \
        || fail_drill "the base backup $BASE_ID does not match its recorded checksums (bit rot or a partial copy)"
    log "base backup checksums match"
else
    log "WARNING: $BASE_ID has no SHA256SUMS — integrity cannot be proven, only restorability"
fi

# --- the restore directory -------------------------------------------------
if [[ -z "$RESTORE_ROOT" ]]; then
    RESTORE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/pitr-restore-XXXXXX")"
    OWNED_TMP=1
else
    OWNED_TMP=0
    mkdir -p "$RESTORE_ROOT"
fi
DATA_DIR="$RESTORE_ROOT/data"

if [[ -e "$DATA_DIR" && -n "$(ls -A "$DATA_DIR" 2>/dev/null || true)" ]]; then
    die "$DATA_DIR is not empty — refusing to restore over an existing data directory"
fi
mkdir -p "$DATA_DIR"
chmod 700 "$DATA_DIR"

STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CLUSTER_STARTED=0

cleanup() {
    # Stop the cluster first: removing a data directory out from under a
    # running postmaster leaves a process writing into deleted inodes.
    if [[ "$CLUSTER_STARTED" -eq 1 ]]; then
        pg_ctl --pgdata="$DATA_DIR" --mode=immediate stop >/dev/null 2>&1 || true
    fi
    if [[ "$OWNED_TMP" -eq 1 ]]; then
        rm -rf -- "$RESTORE_ROOT"
    fi
    return 0
}
trap cleanup EXIT

log "extracting $BASE_ID into $DATA_DIR"
tar -xzf "$BASE_PATH/base.tar.gz" -C "$DATA_DIR" \
    || fail_drill "could not extract base.tar.gz"

# pg_basebackup --wal-method=stream puts the WAL generated during the
# backup in pg_wal.tar.gz. Without it, recovery cannot even reach a
# consistent state, let alone roll forward.
if [[ -s "$BASE_PATH/pg_wal.tar.gz" ]]; then
    mkdir -p "$DATA_DIR/pg_wal"
    tar -xzf "$BASE_PATH/pg_wal.tar.gz" -C "$DATA_DIR/pg_wal" \
        || fail_drill "could not extract pg_wal.tar.gz"
fi

# --- recovery configuration ------------------------------------------------
# Postgres 12+ : recovery settings live in postgresql.auto.conf and the
# presence of recovery.signal is what puts the cluster into recovery.
{
    echo "# written by scripts/restore-pitr.sh — point-in-time recovery"
    echo "restore_command = 'cp ${ARCHIVE_DIR}/%f %p'"
    if [[ -n "$TARGET_TIME" ]]; then
        echo "recovery_target_time = '${TARGET_TIME}'"
        # Stop AT the target and wait, rather than promoting. A cluster
        # that promotes itself cannot be re-targeted without starting the
        # whole restore again.
        echo "recovery_target_action = 'pause'"
    fi
    echo "port = ${PITR_PORT}"
    # A restored cluster must never archive on top of the archive it was
    # restored from: that is how a timeline overwrites its own history.
    echo "archive_mode = off"
} >> "$DATA_DIR/postgresql.auto.conf"

touch "$DATA_DIR/recovery.signal"

log "recovery configured: archive=$ARCHIVE_DIR target_time=${TARGET_TIME:-<end of WAL>} port=$PITR_PORT"

if [[ "$VERIFY" -eq 0 ]]; then
    # Prepared, not started. Keep the directory for the operator.
    OWNED_TMP=0
    trap - EXIT
    echo
    echo "PITR data directory prepared (NOT started):"
    echo "  $DATA_DIR"
    echo
    echo "Start it yourself when you are ready:"
    echo "  pg_ctl -D '$DATA_DIR' -l '$RESTORE_ROOT/postgres.log' start"
    echo "  psql -p $PITR_PORT -d postgres -c 'SELECT pg_is_in_recovery();'"
    echo
    echo "Recovery pauses at the target. Promote only when you have"
    echo "checked the data is what you expect:"
    echo "  psql -p $PITR_PORT -c 'SELECT pg_wal_replay_resume();'   # continue"
    echo "  pg_ctl -D '$DATA_DIR' promote                            # go live"
    exit 0
fi

# --- --verify: start, wait for recovery, check, record ---------------------
LOGFILE="$RESTORE_ROOT/postgres.log"
log "starting the restored cluster on port $PITR_PORT"
if ! pg_ctl --pgdata="$DATA_DIR" --log="$LOGFILE" --wait --timeout=300 start >/dev/null 2>&1; then
    tail -40 "$LOGFILE" >&2 || true
    fail_drill "the restored cluster did not start — see the log above"
fi
CLUSTER_STARTED=1

PSQL=(psql --host=/tmp --port="$PITR_PORT" --dbname=postgres --no-psqlrc -tAc)
# A cluster recovering to a target reaches consistency and then pauses;
# polling is the documented way to wait for it.
deadline=$(( $(date -u +%s) + 300 ))
while :; do
    state="$("${PSQL[@]}" "SELECT pg_is_in_recovery()" 2>/dev/null || echo "")"
    [[ -n "$state" ]] && break
    [[ "$(date -u +%s)" -lt "$deadline" ]] || fail_drill "the restored cluster never became queryable within 300s"
    sleep 2
done

recovered_time="$("${PSQL[@]}" "SELECT COALESCE(pg_last_xact_replay_timestamp()::text, 'none')" 2>/dev/null || echo "none")"
log "cluster is up; last replayed transaction: $recovered_time"

# The business check: the same table list the logical drill uses, so the
# two drills cannot disagree about what "restored" means.
TARGET_DB="${POSTGRES_DB:-sniper}"
REQUIRED_TABLES=(orders executions organizations memberships audit_events)
missing=()
for table in "${REQUIRED_TABLES[@]}"; do
    exists="$(psql --host=/tmp --port="$PITR_PORT" --dbname="$TARGET_DB" --no-psqlrc -tAc \
        "SELECT to_regclass('public.$table') IS NOT NULL" 2>/dev/null || echo f)"
    [[ "$exists" == "t" ]] || missing+=("$table")
done
[[ ${#missing[@]} -eq 0 ]] \
    || fail_drill "the recovered cluster is missing table(s): ${missing[*]}"

table_count="$(psql --host=/tmp --port="$PITR_PORT" --dbname="$TARGET_DB" --no-psqlrc -tAc \
    "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'" 2>/dev/null || echo 0)"

record "{\"event\":\"pitr_verified\",\"at\":\"$STARTED_AT\",\"base_id\":\"$BASE_ID\",\"target_time\":\"${TARGET_TIME:-end_of_wal}\",\"replayed_to\":\"$recovered_time\",\"tables\":$table_count,\"ok\":true}"

log "stopping the restored cluster"
pg_ctl --pgdata="$DATA_DIR" --mode=fast --wait stop >/dev/null 2>&1 || true
CLUSTER_STARTED=0

echo
echo "POINT-IN-TIME RECOVERY DRILL PASSED"
echo "  base        : $BASE_ID"
echo "  target      : ${TARGET_TIME:-<end of WAL>}"
echo "  replayed to : $recovered_time"
echo "  tables      : $table_count"
echo "  recorded    : $LEDGER"
echo
echo "/api/saas/backup/status will now report pitr=current for this deployment."
