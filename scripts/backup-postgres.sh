#!/usr/bin/env bash
# ============================================================================
# scripts/backup-postgres.sh — take, checksum, encrypt, prune and RECORD a
# PostgreSQL backup (P1, SLO D1).
#
#   ./scripts/backup-postgres.sh                 # take a backup now
#   ./scripts/backup-postgres.sh --dry-run       # show what would happen
#
# This is the thing `docs/BACKUP-RESTORE.md` used to describe with the
# words "Cron this." Documentation that ends in an instruction to a human
# is not a backup system: nothing in this repository took a backup, while
# `/api/saas/backup/status` told customers their data was protected.
#
# Every run appends one record to the ledger that
# `crate::ops::backup_ledger` reads, including FAILED runs — a backup
# system that goes quiet when it breaks is worse than none, because the
# silence reads as success.
#
# Environment (see deploy/environments/*.env.template):
#   BACKUP_DIR              where dumps are written       (default ./deploy/backups)
#   BACKUP_LEDGER_PATH      the JSONL ledger              (default $BACKUP_DIR/backups.jsonl)
#   BACKUP_RETENTION_DAYS   prune dumps older than this   (default 30)
#   BACKUP_AGE_RECIPIENT    age public key; when set, dumps are encrypted
#   DATABASE_URL            postgres://…                  (required)
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/deploy/backups}"
LEDGER="${BACKUP_LEDGER_PATH:-$BACKUP_DIR/backups.jsonl}"
RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-30}"
AGE_RECIPIENT="${BACKUP_AGE_RECIPIENT:-}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
BACKUP_ID="pg-$(date -u +%Y%m%dT%H%M%SZ)"

log()  { echo "[$(date -u +%H:%M:%S)] $*"; }
die()  { echo "FAIL: $*" >&2; record_failure "$*"; exit 1; }

# --- the ledger ------------------------------------------------------------
# Written even on failure. `ok:false` is the signal the status endpoint
# turns into `state: failing`, which is the whole point: the operator and
# the customer both learn that the schedule is broken.
record() {
    local line="$1"
    mkdir -p "$(dirname "$LEDGER")"
    # One atomic append. Concurrent writers are prevented by the lock
    # below, but `>>` on a line shorter than PIPE_BUF is atomic anyway.
    printf '%s\n' "$line" >> "$LEDGER"
}

record_failure() {
    # A dry run makes no claim about the state of the backups and must
    # never write to the ledger: a rehearsal that marks the deployment
    # "failing" would be worse than not rehearsing.
    [[ "$DRY_RUN" -eq 1 ]] && return 0
    local reason="${1//\"/\'}"
    record "{\"event\":\"backup\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"backup_id\":\"$BACKUP_ID\",\"ok\":false,\"detail\":\"${reason}\"}"
}

# --- preconditions ---------------------------------------------------------
# On a real run an unmet precondition is fatal AND recorded. Under
# --dry-run it is reported and execution continues, so the rehearsal can
# check the whole script on a host (a CI runner, a laptop) that was never
# going to be able to take a backup in the first place.
DRY_PROBLEMS=0

# require <message> <command...> — run the command; on failure die (real
# run) or report and continue (dry run). The command is passed as
# arguments rather than evaluated beforehand on purpose: under `set -e` a
# bare failing `command -v pg_dump` aborts the script before any handler
# could run, which silently turned a readable error into an empty exit 1.
require() {
    local msg="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        return 0
    fi
    if [[ "$DRY_RUN" -eq 1 ]]; then
        DRY_PROBLEMS=$((DRY_PROBLEMS + 1))
        log "DRY RUN: would FAIL — $msg"
        # Returns 0 on purpose: a non-zero return here would abort the
        # rehearsal at the first unmet precondition under `set -e`, and
        # the operator would only ever see one problem per run.
        return 0
    fi
    die "$msg"
}

require "DATABASE_URL is not set" test -n "${DATABASE_URL:-}"
require "pg_dump is not installed on this host" command -v pg_dump
require "sha256sum is not available" command -v sha256sum

ENCRYPTED=false
if [[ -n "$AGE_RECIPIENT" ]]; then
    # Fail CLOSED. If encryption was requested and the tool is missing,
    # writing a plaintext dump of a trading database to disk and calling
    # it a success is not an acceptable fallback.
    require "BACKUP_AGE_RECIPIENT is set but 'age' is not installed — refusing to write an unencrypted dump" \
        command -v age
    ENCRYPTED=true
else
    log "WARNING: BACKUP_AGE_RECIPIENT is not set — the dump will be written UNENCRYPTED."
    log "         Acceptable only when the destination volume is itself encrypted at rest."
fi

mkdir -p "$BACKUP_DIR"

# --- one at a time ---------------------------------------------------------
# Two overlapping pg_dumps against the same database waste I/O and can
# interleave their ledger records; a missed run is better than a confused
# one, and the next scheduled run recovers it.
LOCK="$BACKUP_DIR/.backup.lock"
exec 9>"$LOCK"
flock -n 9 || die "another backup is already running (lock: $LOCK)"

# --- dump ------------------------------------------------------------------
DUMP="$BACKUP_DIR/$BACKUP_ID.dump"
[[ "$ENCRYPTED" == true ]] && DUMP="$DUMP.age"

if [[ "$DRY_RUN" -eq 1 ]]; then
    log "DRY RUN: would write $DUMP"
    log "DRY RUN: retention ${RETENTION_DAYS}d, ledger $LEDGER, encrypted=$ENCRYPTED"
    log "DRY RUN: would prune *.dump/*.dump.age older than ${RETENTION_DAYS}d in $BACKUP_DIR"
    log "DRY RUN: no dump was written and the ledger was NOT touched (the backup directory and its lock file were created, which also proves the destination is writable)"
    if [[ "$DRY_PROBLEMS" -gt 0 ]]; then
        log "DRY RUN: $DRY_PROBLEMS precondition(s) unmet — a real run on THIS host would fail"
    fi
    # Exit 0 regardless: a rehearsal reports, it does not gate. The exit
    # code of a real run is what the ledger and the status endpoint key on.
    exit 0
fi

log "dumping to $DUMP"
# -Fc: custom format — compressed, and pg_restore can select tables.
# The dump includes _sqlx_migrations, so schema version travels with the
# data and the restore drill can assert the high-water mark.
if [[ "$ENCRYPTED" == true ]]; then
    # PIPEFAIL is on: a pg_dump failure is not masked by a successful age.
    pg_dump --dbname="$DATABASE_URL" --format=custom --no-owner --no-privileges \
        | age --recipient "$AGE_RECIPIENT" --output "$DUMP" \
        || die "pg_dump | age failed"
else
    pg_dump --dbname="$DATABASE_URL" --format=custom --no-owner --no-privileges \
        --file "$DUMP" || die "pg_dump failed"
fi

[[ -s "$DUMP" ]] || die "the dump file is empty"

SIZE_BYTES="$(stat -c %s "$DUMP")"
SHA256="$(sha256sum "$DUMP" | cut -d' ' -f1)"

# A dump far smaller than the previous one usually means the database was
# empty, the wrong database was named, or the dump was truncated. Report
# it; do not silently accept it as the new baseline.
PREVIOUS_SIZE="$(ls -t "$BACKUP_DIR"/*.dump "$BACKUP_DIR"/*.dump.age 2>/dev/null \
    | grep -v "$BACKUP_ID" | head -1 | xargs -r stat -c %s 2>/dev/null || true)"
if [[ -n "$PREVIOUS_SIZE" && "$PREVIOUS_SIZE" -gt 0 ]]; then
    if [[ "$SIZE_BYTES" -lt $(( PREVIOUS_SIZE / 2 )) ]]; then
        log "WARNING: this dump ($SIZE_BYTES B) is less than half the previous one ($PREVIOUS_SIZE B)"
    fi
fi

log "wrote $DUMP ($SIZE_BYTES bytes, sha256 ${SHA256:0:12}…)"

# --- prune -----------------------------------------------------------------
# Pruning happens AFTER a successful dump, never before: a failed backup
# must not also delete the last good copy.
PRUNED=0
if [[ "$RETENTION_DAYS" -gt 0 ]]; then
    while IFS= read -r -d '' old; do
        rm -f -- "$old"
        PRUNED=$(( PRUNED + 1 ))
        log "pruned $(basename "$old")"
    done < <(find "$BACKUP_DIR" -maxdepth 1 -type f \( -name '*.dump' -o -name '*.dump.age' \) \
                -mtime +"$RETENTION_DAYS" -print0)
fi

# --- record ----------------------------------------------------------------
record "{\"event\":\"backup\",\"at\":\"$STARTED_AT\",\"backup_id\":\"$BACKUP_ID\",\"sha256\":\"$SHA256\",\"size_bytes\":$SIZE_BYTES,\"encrypted\":$ENCRYPTED,\"retention_days\":$RETENTION_DAYS,\"pruned\":$PRUNED,\"ok\":true}"

log "recorded in $LEDGER"
echo
echo "backup_id : $BACKUP_ID"
echo "encrypted : $ENCRYPTED"
echo "retention : ${RETENTION_DAYS}d (pruned $PRUNED old file(s))"
echo
echo "A backup nobody has restored is an assumption. Run the drill:"
echo "  ./scripts/verify-backup-restore.sh $BACKUP_ID"
