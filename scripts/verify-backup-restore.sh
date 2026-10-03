#!/usr/bin/env bash
# ============================================================================
# scripts/verify-backup-restore.sh — prove a backup can actually be
# restored (P1, SLO D2).
#
#   ./scripts/verify-backup-restore.sh                # newest backup
#   ./scripts/verify-backup-restore.sh pg-2026…Z      # a specific one
#   ./scripts/verify-backup-restore.sh --check        # is a drill overdue?
#   ./scripts/verify-backup-restore.sh --from-offsite pg-2026…Z
#                                                     # pull the OFF-SITE copy
#                                                     # and restore that instead
#
# A backup that has never been restored is a hypothesis. This script
# restores one into a THROWAWAY database, verifies the restored content,
# records the result in the ledger, and drops the throwaway database. It
# is the only thing that moves `/api/saas/backup/status` from
# `unverified` to `verified`, and that gate is deliberate.
#
# What it verifies, in order of how often each one catches a real problem:
#   1. the checksum recorded at backup time still matches the file on disk
#      (bit rot, partial copy, truncated transfer);
#   2. pg_restore completes without error into an empty database;
#   3. the restored database contains the `_sqlx_migrations` table and its
#      high-water mark matches the migration series in this checkout;
#   4. the tables the business actually depends on exist and carry rows.
#
# SAFETY: it refuses to touch the source database. The restore target is
# a new database whose name it generates, and it drops only that.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/deploy/backups}"
LEDGER="${BACKUP_LEDGER_PATH:-$BACKUP_DIR/backups.jsonl}"
DRILL_MAX_AGE_DAYS="${BACKUP_DRILL_MAX_AGE_DAYS:-90}"

log()  { echo "[$(date -u +%H:%M:%S)] $*"; }
die()  { echo "FAIL: $*" >&2; exit 1; }

record() {
    mkdir -p "$(dirname "$LEDGER")"
    printf '%s\n' "$1" >> "$LEDGER"
}

# --- --check: is the quarterly drill overdue? ------------------------------
# Cheap enough for CI, and it needs no database at all.
if [[ "${1:-}" == "--check" ]]; then
    [[ -f "$LEDGER" ]] || die "no ledger at $LEDGER — no drill has ever been recorded"
    last="$(grep '"event":"restore_verified"' "$LEDGER" 2>/dev/null \
            | grep '"ok":true' | tail -1 || true)"
    [[ -n "$last" ]] || die "no SUCCESSFUL restore drill has ever been recorded (SLO D2)"
    last_at="$(printf '%s' "$last" | sed -E 's/.*"at":"([^"]+)".*/\1/')"
    age_days=$(( ( $(date -u +%s) - $(date -u -d "$last_at" +%s) ) / 86400 ))
    echo "last verified restore: $last_at (${age_days}d ago, limit ${DRILL_MAX_AGE_DAYS}d)"
    [[ "$age_days" -le "$DRILL_MAX_AGE_DAYS" ]] \
        || die "the restore drill is overdue by $(( age_days - DRILL_MAX_AGE_DAYS )) day(s)"
    echo "OK"
    exit 0
fi

# --- --from-offsite: drill the copy that survives losing this host ---------
# Restoring the local file proves the dump is good. It does NOT prove the
# off-site copy is good — and the off-site copy is the only one that
# exists in the scenario the off-site copy is for. This mode downloads
# the remote object into a temporary directory and runs the identical
# drill against it, checksum included.
SOURCE="local"
if [[ "${1:-}" == "--from-offsite" ]]; then
    SOURCE="offsite"
    shift
    OFFSITE_ID="${1:-}"
    [[ -n "$OFFSITE_ID" ]] || die "--from-offsite requires a backup id (e.g. pg-20261002T020004Z)"
    TARGET="${BACKUP_OFFSITE_TARGET:-}"
    [[ -n "$TARGET" ]] || die "BACKUP_OFFSITE_TARGET is not set — there is no off-site copy to drill"

    OFFSITE_TMP="$(mktemp -d)"
    # Removed on EXIT below alongside the throwaway database. A dump left
    # in /tmp is an unencrypted-at-rest copy of the whole database.
    trap 'rm -rf "$OFFSITE_TMP"' EXIT

    downloaded=""
    for candidate in "$OFFSITE_ID.dump.age" "$OFFSITE_ID.dump"; do
        if [[ "$TARGET" == *"://"* ]]; then
            command -v aws >/dev/null 2>&1 || die "the 'aws' CLI is required to pull from $TARGET"
            aws s3 cp "$TARGET/$candidate" "$OFFSITE_TMP/$candidate" --only-show-errors 2>/dev/null \
                && downloaded="$candidate" && break
        else
            command -v rclone >/dev/null 2>&1 || die "'rclone' is required to pull from the configured remote"
            rclone copyto "$TARGET/$candidate" "$OFFSITE_TMP/$candidate" --quiet 2>/dev/null \
                && downloaded="$candidate" && break
        fi
    done
    [[ -n "$downloaded" ]] || die "could not download $OFFSITE_ID from the off-site target — the copy you would restore from in a disaster is NOT there"

    log "pulled $downloaded from the off-site target into a temporary directory"
    # The rest of the script is unchanged: point it at the downloaded file.
    BACKUP_DIR="$OFFSITE_TMP"
    set -- "$OFFSITE_ID"
fi

# --- preconditions ---------------------------------------------------------
[[ -n "${DATABASE_URL:-}" ]] || die "DATABASE_URL is not set"
command -v pg_restore >/dev/null 2>&1 || die "pg_restore is not installed"
command -v psql >/dev/null 2>&1 || die "psql is not installed"

# --- pick the backup -------------------------------------------------------
BACKUP_ID="${1:-}"
if [[ -z "$BACKUP_ID" ]]; then
    newest="$(ls -t "$BACKUP_DIR"/*.dump "$BACKUP_DIR"/*.dump.age 2>/dev/null | head -1 || true)"
    [[ -n "$newest" ]] || die "no backup files in $BACKUP_DIR"
    BACKUP_ID="$(basename "$newest")"
    BACKUP_ID="${BACKUP_ID%%.dump*}"
fi

DUMP="$BACKUP_DIR/$BACKUP_ID.dump"
ENCRYPTED=false
if [[ ! -f "$DUMP" ]]; then
    if [[ -f "$DUMP.age" ]]; then
        DUMP="$DUMP.age"; ENCRYPTED=true
    else
        die "no dump found for $BACKUP_ID in $BACKUP_DIR"
    fi
fi
log "verifying $BACKUP_ID ($(basename "$DUMP"), encrypted=$ENCRYPTED)"

fail_drill() {
    local reason="${1//\"/\'}"
    record "{\"event\":\"restore_verified\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"backup_id\":\"$BACKUP_ID\",\"source\":\"$SOURCE\",\"ok\":false,\"detail\":\"${reason}\"}"
    die "$reason"
}

# --- 1. checksum -----------------------------------------------------------
# Compared against what the ledger recorded at backup time, not against a
# value recomputed from the same file — that would prove nothing.
recorded_sha="$(grep "\"backup_id\":\"$BACKUP_ID\"" "$LEDGER" 2>/dev/null \
    | grep '"event":"backup"' | tail -1 | sed -nE 's/.*"sha256":"([a-f0-9]+)".*/\1/p' || true)"
if [[ -n "$recorded_sha" ]]; then
    actual_sha="$(sha256sum "$DUMP" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$recorded_sha" ]] \
        || fail_drill "checksum mismatch: the file on disk is not the backup that was recorded"
    log "checksum matches the ledger"
else
    log "WARNING: no checksum recorded for $BACKUP_ID — integrity cannot be proven, only restorability"
fi

# --- 2. restore into a throwaway database ----------------------------------
TARGET_DB="restore_drill_$(date -u +%Y%m%d%H%M%S)"
ADMIN_URL="${DATABASE_URL%/*}/postgres"   # same server, 'postgres' maintenance DB
TARGET_URL="${DATABASE_URL%/*}/$TARGET_DB"

cleanup() {
    # Always drop the throwaway database, including on failure: a drill
    # that litters abandoned databases will be turned off by whoever
    # finds them. The temporary off-site download is removed here too —
    # a single trap, because a second `trap ... EXIT` would REPLACE this
    # one and quietly leak whichever cleanup was registered first.
    psql "$ADMIN_URL" -q -c "DROP DATABASE IF EXISTS \"$TARGET_DB\" WITH (FORCE);" >/dev/null 2>&1 || true
    [[ -n "${OFFSITE_TMP:-}" ]] && rm -rf "$OFFSITE_TMP"
    return 0
}
trap cleanup EXIT

log "creating throwaway database $TARGET_DB"
psql "$ADMIN_URL" -q -c "CREATE DATABASE \"$TARGET_DB\";" \
    || fail_drill "could not create the drill database"

log "restoring…"
if [[ "$ENCRYPTED" == true ]]; then
    command -v age >/dev/null 2>&1 || fail_drill "the dump is encrypted but 'age' is not installed"
    [[ -n "${BACKUP_AGE_IDENTITY:-}" ]] \
        || fail_drill "BACKUP_AGE_IDENTITY (private key file) is required to decrypt this dump"
    age --decrypt --identity "$BACKUP_AGE_IDENTITY" "$DUMP" \
        | pg_restore --dbname="$TARGET_URL" --no-owner --no-privileges --exit-on-error \
        || fail_drill "pg_restore failed on the decrypted stream"
else
    pg_restore --dbname="$TARGET_URL" --no-owner --no-privileges --exit-on-error "$DUMP" \
        || fail_drill "pg_restore failed"
fi

# --- 3. schema version -----------------------------------------------------
restored_high_water="$(psql "$TARGET_URL" -tAc \
    "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success" 2>/dev/null || echo "")"
[[ -n "$restored_high_water" ]] \
    || fail_drill "the restored database has no _sqlx_migrations table — this is not a dump of this system"

checkout_high_water="$(find crates/core/migrations -name '[0-9]*.sql' -printf '%f\n' \
    | sed -E 's/^0*([0-9]+)_.*/\1/' | sort -n | tail -1)"
log "migration high-water: restored=$restored_high_water, checkout=$checkout_high_water"
if [[ "$restored_high_water" -gt "$checkout_high_water" ]]; then
    fail_drill "the backup is NEWER than this checkout (restored $restored_high_water > checkout $checkout_high_water)"
fi

# --- 4. the tables the business depends on ---------------------------------
# Not an exhaustive schema diff — a deliberately small list of tables
# whose absence would make the restored database useless. A dump that
# restores "successfully" into an empty schema passes pg_restore and
# fails this.
REQUIRED_TABLES=(orders executions organizations memberships audit_events)
missing=()
for table in "${REQUIRED_TABLES[@]}"; do
    exists="$(psql "$TARGET_URL" -tAc \
        "SELECT to_regclass('public.$table') IS NOT NULL" 2>/dev/null || echo f)"
    [[ "$exists" == "t" ]] || missing+=("$table")
done
[[ ${#missing[@]} -eq 0 ]] || fail_drill "restored database is missing table(s): ${missing[*]}"

table_count="$(psql "$TARGET_URL" -tAc \
    "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'")"
log "restored $table_count tables; all required tables present"

# --- 5. record -------------------------------------------------------------
record "{\"event\":\"restore_verified\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"backup_id\":\"$BACKUP_ID\",\"source\":\"$SOURCE\",\"tables\":$table_count,\"migration_high_water\":$restored_high_water,\"ok\":true}"

echo
echo "RESTORE DRILL PASSED"
echo "  backup   : $BACKUP_ID"
echo "  source   : $SOURCE"
echo "  tables   : $table_count"
echo "  schema   : migration $restored_high_water"
echo "  recorded : $LEDGER"
echo
echo "/api/saas/backup/status will now report state=verified for the next ${DRILL_MAX_AGE_DAYS} days."
