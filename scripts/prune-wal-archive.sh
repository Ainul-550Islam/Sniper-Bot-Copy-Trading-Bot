#!/usr/bin/env bash
# ============================================================================
# scripts/prune-wal-archive.sh — delete WAL segments no surviving base
# backup needs (P1 batch 5).
#
# WHY IT IS A SEPARATE SCRIPT
#
# This is the one operation in the whole backup system that can destroy
# recoverability while every job keeps reporting success. Deleting a WAL
# segment that the oldest retained base backup still needs leaves a base
# that cannot roll forward — a backup that exists, checksums fine, and
# restores to nothing. So the deletion lives on its own, with its own
# rule, written out in one place:
#
#   NEVER delete a segment newer than the START of the oldest base
#   backup still on disk.
#
# `pg_archivecleanup` implements exactly that and ships with PostgreSQL;
# this script works out the cut-off and refuses to guess when it cannot.
#
# USAGE
#   ./scripts/prune-wal-archive.sh
#   ./scripts/prune-wal-archive.sh --dry-run
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/deploy/backups}"
BASE_DIR="${BACKUP_BASE_DIR:-$BACKUP_DIR/base}"
ARCHIVE_DIR="${BACKUP_WAL_ARCHIVE_DIR:-$BACKUP_DIR/wal-archive}"
LEDGER="${BACKUP_LEDGER_PATH:-$BACKUP_DIR/backups.jsonl}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

log() { echo "[$(date -u +%H:%M:%S)] $*"; }
die() { echo "FAIL: $*" >&2; exit 1; }

record() {
    [[ "$DRY_RUN" -eq 1 ]] && return 0
    mkdir -p "$(dirname "$LEDGER")"
    printf '%s\n' "$1" >> "$LEDGER"
}

[[ -d "$ARCHIVE_DIR" ]] || { log "no WAL archive at $ARCHIVE_DIR — nothing to prune"; exit 0; }

# --- the cut-off -----------------------------------------------------------
# The oldest base backup on disk defines it. If there is no base backup,
# there is nothing that can replay this WAL — and that is a reason to
# STOP, not to delete: the archive may be the only evidence left of
# transactions whose base backup was lost.
OLDEST_BASE="$(find "$BASE_DIR" -maxdepth 1 -mindepth 1 -type d -name 'base-*' -printf '%f\n' 2>/dev/null | sort | head -1)"
if [[ -z "$OLDEST_BASE" ]]; then
    die "no base backup in $BASE_DIR — refusing to prune WAL that nothing can currently replay"
fi

# pg_basebackup writes backup_manifest inside base.tar.gz; the simplest
# reliable source for the starting segment is the backup label file that
# `-Ft` leaves alongside it when present, otherwise the manifest inside
# the tar. Reading one 16 MB tar header is cheap compared to the cost of
# guessing wrong.
START_SEGMENT=""
if [[ -f "$BASE_DIR/$OLDEST_BASE/backup_label" ]]; then
    START_SEGMENT="$(sed -n 's/^START WAL LOCATION:.*file \([0-9A-F]\{24\}\)).*/\1/p' "$BASE_DIR/$OLDEST_BASE/backup_label")"
fi
if [[ -z "$START_SEGMENT" && -s "$BASE_DIR/$OLDEST_BASE/base.tar.gz" ]]; then
    START_SEGMENT="$(tar -xzOf "$BASE_DIR/$OLDEST_BASE/base.tar.gz" backup_label 2>/dev/null \
        | sed -n 's/^START WAL LOCATION:.*file \([0-9A-F]\{24\}\)).*/\1/p' || true)"
fi

[[ -n "$START_SEGMENT" ]] \
    || die "could not determine the starting WAL segment of $OLDEST_BASE — refusing to prune on a guess"

log "oldest retained base: $OLDEST_BASE (starts at WAL segment $START_SEGMENT)"

BEFORE="$(find "$ARCHIVE_DIR" -maxdepth 1 -type f -name '[0-9A-F]*' | wc -l)"

if [[ "$DRY_RUN" -eq 1 ]]; then
    if command -v pg_archivecleanup >/dev/null 2>&1; then
        log "DRY RUN: pg_archivecleanup would remove:"
        pg_archivecleanup -n "$ARCHIVE_DIR" "$START_SEGMENT" || true
    else
        log "DRY RUN: pg_archivecleanup is not installed on this host"
    fi
    log "DRY RUN: $BEFORE segment(s) currently archived; the ledger was NOT touched"
    exit 0
fi

command -v pg_archivecleanup >/dev/null 2>&1 \
    || die "pg_archivecleanup is not installed — it ships with PostgreSQL and this script will not hand-roll segment arithmetic"

pg_archivecleanup "$ARCHIVE_DIR" "$START_SEGMENT" \
    || die "pg_archivecleanup failed"

AFTER="$(find "$ARCHIVE_DIR" -maxdepth 1 -type f -name '[0-9A-F]*' | wc -l)"
REMOVED=$(( BEFORE - AFTER ))

record "{\"event\":\"wal_pruned\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"removed\":$REMOVED,\"remaining\":$AFTER,\"oldest_base\":\"$OLDEST_BASE\",\"ok\":true}"

log "pruned $REMOVED segment(s); $AFTER remain, all of them replayable onto $OLDEST_BASE"
