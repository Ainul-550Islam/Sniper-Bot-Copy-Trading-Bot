#!/usr/bin/env bash
# ============================================================================
# scripts/backup-basebackup.sh — take the PITR base backup (P1 batch 5).
#
# A WAL archive on its own restores nothing. Point-in-time recovery needs
# a physical BASE backup to replay the archived segments onto, and the
# oldest WAL you may delete is the one the newest usable base needs. So:
#
#   pg_dump          → logical, portable, per-table; the everyday restore
#   pg_basebackup    → physical, byte-for-byte; the PITR starting point
#
# They are not alternatives, and this script does not replace
# scripts/backup-postgres.sh. A cluster with only a base backup can be
# rolled forward but cannot be restored table-by-table into a different
# major version; a cluster with only dumps cannot roll forward at all.
#
# USAGE
#
#   PGHOST=postgres PGUSER=sniper PGPASSWORD=… ./scripts/backup-basebackup.sh
#   ./scripts/backup-basebackup.sh --dry-run
#
# REPLICATION PRIVILEGES: pg_basebackup needs a role with REPLICATION (or
# superuser) and a `replication` entry in pg_hba.conf. In the compose
# stack the default superuser already has both. In a managed service they
# often do NOT — which is exactly the kind of thing that is discovered at
# the wrong time, so the script says so explicitly when it fails.
#
# RETENTION: base backups are pruned only AFTER a newer one succeeds, and
# WAL segments older than the oldest retained base are pruned by
# scripts/prune-wal-archive.sh — never by this script. Deleting WAL that
# the surviving base still needs turns a backup into a decoration.
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

BASE_DIR="${BACKUP_BASE_DIR:-${BACKUP_DIR:-$REPO_ROOT/deploy/backups}/base}"
LEDGER="${BACKUP_LEDGER_PATH:-${BACKUP_DIR:-$REPO_ROOT/deploy/backups}/backups.jsonl}"
RETAIN_BASES="${BACKUP_BASE_RETAIN:-2}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

BASE_ID="base-$(date -u +%Y%m%dT%H%M%SZ)"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

log() { echo "[$(date -u +%H:%M:%S)] $*"; }

record() {
    [[ "$DRY_RUN" -eq 1 ]] && return 0
    mkdir -p "$(dirname "$LEDGER")"
    printf '%s\n' "$1" >> "$LEDGER"
}

record_failure() {
    local reason="${1//\"/\'}"
    record "{\"event\":\"basebackup\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"base_id\":\"$BASE_ID\",\"ok\":false,\"detail\":\"${reason}\"}"
}

die() { echo "FAIL: $*" >&2; record_failure "$*"; exit 1; }

DRY_PROBLEMS=0
require() {
    local msg="$1"; shift
    if "$@" >/dev/null 2>&1; then return 0; fi
    if [[ "$DRY_RUN" -eq 1 ]]; then
        DRY_PROBLEMS=$(( DRY_PROBLEMS + 1 ))
        log "DRY RUN: would FAIL — $msg"
        return 0
    fi
    die "$msg"
}

require "pg_basebackup is not installed on this host" command -v pg_basebackup
require "sha256sum is not available" command -v sha256sum
if [[ -z "${PGHOST:-}" && -z "${DATABASE_URL:-}" ]]; then
    require "neither PGHOST nor DATABASE_URL is set — pg_basebackup needs a connection" false
fi

# pg_basebackup takes a connection string, not a database URL with a
# database name on the end; the database name is irrelevant for a
# physical backup, but passing one is harmless and keeps configuration in
# one place.
CONN=()
[[ -n "${DATABASE_URL:-}" ]] && CONN=(--dbname="$DATABASE_URL")

mkdir -p "$BASE_DIR"
TARGET="$BASE_DIR/$BASE_ID"

if [[ "$DRY_RUN" -eq 1 ]]; then
    log "DRY RUN: would write $TARGET/base.tar.gz (and pg_wal.tar.gz)"
    log "DRY RUN: would keep the $RETAIN_BASES most recent base backup(s), pruning older ones AFTER success"
    log "DRY RUN: ledger $LEDGER was NOT touched"
    [[ "$DRY_PROBLEMS" -gt 0 ]] && log "DRY RUN: $DRY_PROBLEMS precondition(s) unmet — a real run on THIS host would fail"
    exit 0
fi

mkdir -p "$TARGET"

# --- the base backup -------------------------------------------------------
# -Ft -z        : tar, gzip — one file per tablespace, portable to restore
# -X stream     : stream the WAL generated DURING the backup into
#                 pg_wal.tar.gz, so the base is self-contained and can
#                 start even if the archive is momentarily behind
# --checkpoint=fast : do not wait up to checkpoint_timeout to start
# -P            : progress to stderr, which ends up in the job log
log "taking base backup into $TARGET (this copies the whole cluster)"
if ! pg_basebackup "${CONN[@]+"${CONN[@]}"}" \
        --pgdata="$TARGET" \
        --format=tar --gzip --wal-method=stream \
        --checkpoint=fast --progress --no-password; then
    rm -rf -- "$TARGET"
    die "pg_basebackup failed — check that the role has REPLICATION rights and that pg_hba.conf has a 'replication' entry for it"
fi

[[ -s "$TARGET/base.tar.gz" ]] || { rm -rf -- "$TARGET"; die "pg_basebackup produced no base.tar.gz"; }

# --- checksum manifest -----------------------------------------------------
# Written inside the base directory so the pair always travels together.
( cd "$TARGET" && sha256sum ./*.tar.gz > SHA256SUMS ) \
    || die "could not checksum the base backup"

SIZE_BYTES="$(du -sb "$TARGET" | cut -f1)"
BASE_SHA="$(sha256sum "$TARGET/base.tar.gz" | cut -d' ' -f1)"

log "base backup complete: $SIZE_BYTES bytes, base.tar.gz sha256 ${BASE_SHA:0:12}…"

# --- prune older bases, AFTER this one succeeded ---------------------------
PRUNED=0
if [[ "$RETAIN_BASES" -gt 0 ]]; then
    mapfile -t BASES < <(find "$BASE_DIR" -maxdepth 1 -mindepth 1 -type d -name 'base-*' -printf '%f\n' | sort -r)
    index=0
    for b in "${BASES[@]}"; do
        index=$(( index + 1 ))
        [[ "$index" -le "$RETAIN_BASES" ]] && continue
        rm -rf -- "${BASE_DIR:?}/$b"
        PRUNED=$(( PRUNED + 1 ))
        log "pruned old base backup $b"
    done
fi

# The oldest base still on disk defines the oldest WAL segment that must
# be kept. prune-wal-archive.sh reads exactly this value, which is why it
# is recorded rather than recomputed somewhere else.
OLDEST_BASE="$(find "$BASE_DIR" -maxdepth 1 -mindepth 1 -type d -name 'base-*' -printf '%f\n' | sort | head -1)"

record "{\"event\":\"basebackup\",\"at\":\"$STARTED_AT\",\"base_id\":\"$BASE_ID\",\"sha256\":\"$BASE_SHA\",\"size_bytes\":$SIZE_BYTES,\"retained\":$RETAIN_BASES,\"pruned\":$PRUNED,\"oldest_base\":\"$OLDEST_BASE\",\"ok\":true}"

echo
echo "base_id      : $BASE_ID"
echo "size         : $SIZE_BYTES bytes"
echo "retained     : $RETAIN_BASES base backup(s), pruned $PRUNED"
echo "oldest base  : $OLDEST_BASE  (WAL older than this base is prunable)"
echo
echo "A base backup plus an unverified WAL archive is still a hypothesis."
echo "Prove the pair restores to a point in time:"
echo "  ./scripts/restore-pitr.sh --base $BASE_ID --target-time '$(date -u +%Y-%m-%dT%H:%M:%SZ)' --verify"
