#!/bin/sh
# ============================================================================
# scripts/archive-wal.sh — PostgreSQL `archive_command` target (P1 batch 5).
#
# WHY THIS EXISTS
#
# Until now the smallest unit of recovery in this system was a whole dump.
# With a 24-hour schedule that is a recovery point objective of up to 24
# HOURS OF TRADES. Reconciliation rediscovers fills that happened after
# the backup (docs/BACKUP-RESTORE.md §3), but "the chain will sort it out"
# is not a durability design — it is a hope with a settlement risk.
#
# Write-ahead log archiving closes that gap: every WAL segment Postgres
# fills is copied out, so a restore can roll forward to any moment
# between the last base backup and the last archived segment. The RPO
# stops being the backup interval and becomes the WAL segment interval.
#
# WHY /bin/sh AND NOT BASH
#
# This script runs INSIDE the PostgreSQL container, as the postgres user,
# on every segment switch. `postgres:16-alpine` has no bash. Adding one
# to the database image to run a 100-line script would be a new package,
# a new CVE surface and a new failure mode in the one container that must
# never fail to start. So: POSIX sh only — no [[ ]], no arrays, no
# process substitution. Keep it that way.
#
# HOW POSTGRES CALLS IT
#
#   archive_command = '/repo/scripts/archive-wal.sh %p %f'
#
#   %p  path of the segment to archive, relative to the data directory
#   %f  its file name
#
# CONTRACT WITH POSTGRES — read before changing anything here:
#
#   * Exit 0 ONLY when the segment is safely stored. Postgres deletes or
#     recycles the local segment as soon as this exits 0; a lie here is
#     unrecoverable data loss, not a failed job.
#   * Exit non-zero on ANY doubt. Postgres keeps the segment, logs the
#     failure and retries. Disk fills up — loudly — which is the correct
#     failure mode and the reason pg_wal space must be monitored.
#   * NEVER overwrite an existing archived file with different content.
#     Postgres can legitimately re-invoke the command for a segment it
#     already archived (e.g. after a crash between archive and status
#     update); silently replacing a good segment with a different one
#     breaks the whole chain that follows it.
#   * Be fast. This runs inline with WAL recycling.
#
# Postgres serialises archive_command: exactly one instance runs at a
# time per cluster, so this script needs no lock of its own.
#
# LEDGER
#
# A record per 16 MB segment would drown the ledger that
# /api/saas/backup/status reads, so successes are summarised on a
# heartbeat (BACKUP_WAL_HEARTBEAT_SECS, default 300s) carrying the number
# of segments archived since the previous one. FAILURES are recorded
# immediately and never throttled: the whole point of the file is to make
# a broken archive visible.
# ============================================================================
set -eu

SEGMENT_PATH="${1:-}"
SEGMENT_NAME="${2:-}"

ARCHIVE_DIR="${BACKUP_WAL_ARCHIVE_DIR:-/wal-archive}"
LEDGER="${BACKUP_LEDGER_PATH:-/app/data/backups/backups.jsonl}"
HEARTBEAT_SECS="${BACKUP_WAL_HEARTBEAT_SECS:-300}"
STATE_DIR="$ARCHIVE_DIR/.state"
COUNTER_FILE="$STATE_DIR/segments_since_heartbeat"
HEARTBEAT_FILE="$STATE_DIR/last_heartbeat"

now_iso() { date -u +%Y-%m-%dT%H:%M:%SZ; }

record() {
    # The ledger lives on a volume the app can read. Failing to append
    # must never fail the archive command itself: losing a status line is
    # an observability problem, refusing to archive is a data problem.
    mkdir -p "$(dirname "$LEDGER")" 2>/dev/null || return 0
    printf '%s\n' "$1" >> "$LEDGER" 2>/dev/null || true
}

record_failure() {
    reason=$(printf '%s' "$1" | tr '"' "'")
    record "{\"event\":\"wal_archived\",\"at\":\"$(now_iso)\",\"segments\":0,\"ok\":false,\"detail\":\"${reason}\"}"
}

die() {
    echo "archive-wal: FAIL: $*" >&2
    record_failure "$*"
    # Non-zero tells Postgres to KEEP the segment and retry. This is the
    # safe direction of every failure in this script.
    exit 1
}

if [ -z "$SEGMENT_PATH" ] || [ -z "$SEGMENT_NAME" ]; then
    die "usage: archive-wal.sh <%p source path> <%f segment name>"
fi
[ -f "$SEGMENT_PATH" ] || die "source segment $SEGMENT_PATH does not exist"

# The name comes from Postgres, but this script also runs from the shell
# during drills. A name with a slash would escape the archive directory.
case "$SEGMENT_NAME" in
    */*|*..*|"") die "refusing a segment name that is not a plain file name: $SEGMENT_NAME" ;;
esac

mkdir -p "$ARCHIVE_DIR" "$STATE_DIR" || die "cannot create the archive directory $ARCHIVE_DIR"
[ -w "$ARCHIVE_DIR" ] || die "the archive directory $ARCHIVE_DIR is not writable"

DEST="$ARCHIVE_DIR/$SEGMENT_NAME"
SRC_SHA=$(sha256sum "$SEGMENT_PATH" | cut -d' ' -f1)

# --- already archived? -----------------------------------------------------
# Identical content → success (idempotent re-invocation, which Postgres is
# allowed to do). Different content → hard failure, never an overwrite.
if [ -e "$DEST" ]; then
    DEST_SHA=$(sha256sum "$DEST" | cut -d' ' -f1)
    if [ "$DEST_SHA" = "$SRC_SHA" ]; then
        exit 0
    fi
    die "$SEGMENT_NAME is already archived with DIFFERENT content — refusing to overwrite (this means two clusters are archiving into one directory, or a timeline was reused)"
fi

# --- copy, verify, then publish atomically ---------------------------------
# The temporary name keeps a partially written segment from ever being
# visible under its real name: a restore that picks up a half-copied
# segment fails in the most expensive way possible, during recovery.
TMP="$DEST.partial.$$"
trap 'rm -f -- "$TMP"' EXIT

cp -- "$SEGMENT_PATH" "$TMP" || die "copy of $SEGMENT_NAME failed"

TMP_SHA=$(sha256sum "$TMP" | cut -d' ' -f1)
[ "$TMP_SHA" = "$SRC_SHA" ] \
    || die "checksum mismatch after copying $SEGMENT_NAME — the archive filesystem is not returning what was written"

# Flush before the rename. Without this, a host crash can leave the
# directory entry pointing at a file whose contents were never written.
# `sync -f` is GNU; busybox sync takes no flags, hence the fallback.
if command -v sync >/dev/null 2>&1; then
    sync -f "$TMP" 2>/dev/null || sync 2>/dev/null || true
fi

mv -- "$TMP" "$DEST" || die "could not publish $SEGMENT_NAME into the archive"
[ -f "$DEST" ] || die "$SEGMENT_NAME vanished immediately after being published"

trap - EXIT

# --- throttled heartbeat ---------------------------------------------------
# Counter and timestamp are plain files in the archive's own state
# directory: no database, no daemon, nothing that could itself be down
# while WAL is piling up.
COUNT=0
if [ -f "$COUNTER_FILE" ]; then
    COUNT=$(cat "$COUNTER_FILE" 2>/dev/null || echo 0)
fi
case "$COUNT" in
    ''|*[!0-9]*) COUNT=0 ;;
esac
COUNT=$((COUNT + 1))
printf '%s' "$COUNT" > "$COUNTER_FILE" 2>/dev/null || true

LAST=0
if [ -f "$HEARTBEAT_FILE" ]; then
    LAST=$(cat "$HEARTBEAT_FILE" 2>/dev/null || echo 0)
fi
case "$LAST" in
    ''|*[!0-9]*) LAST=0 ;;
esac
NOW_EPOCH=$(date -u +%s)

if [ $((NOW_EPOCH - LAST)) -ge "$HEARTBEAT_SECS" ]; then
    record "{\"event\":\"wal_archived\",\"at\":\"$(now_iso)\",\"segments\":$COUNT,\"ok\":true}"
    printf '%s' "$NOW_EPOCH" > "$HEARTBEAT_FILE" 2>/dev/null || true
    printf '0' > "$COUNTER_FILE" 2>/dev/null || true
fi

exit 0
