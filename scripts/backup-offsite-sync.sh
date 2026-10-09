#!/usr/bin/env bash
# ============================================================================
# scripts/backup-offsite-sync.sh — copy local dumps to off-site storage
# and record the copy in the backup ledger (P1 batch 4, SLO D1).
#
# WHY THIS EXISTS
#
# scripts/backup-postgres.sh writes its dumps to a volume on the same host
# as the database. That protects against `DROP TABLE`, a bad migration and
# a corrupted page. It protects against NOTHING that takes the host:
# a destroyed VM, a deleted volume, a ransomed filesystem or a lost
# region takes the database and every one of its backups together.
#
# Until this script runs successfully, `/api/saas/backup/status` says so
# in plain words rather than implying geographic redundancy nobody has.
#
# USAGE
#
#   BACKUP_OFFSITE_TARGET=s3://my-bucket/sniper/prod ./scripts/backup-offsite-sync.sh
#   BACKUP_OFFSITE_INCLUDE_PITR=true ./scripts/backup-offsite-sync.sh   # + WAL/base
#   BACKUP_OFFSITE_TARGET=wasabi:bucket/path         ./scripts/backup-offsite-sync.sh   # rclone
#   ./scripts/backup-offsite-sync.sh --dry-run
#
# WHAT GETS COPIED
#
# By default: the logical dumps only. With BACKUP_OFFSITE_INCLUDE_PITR=true
# the physical base backups and the WAL archive go too — and they must,
# if point-in-time recovery is meant to survive losing this host. A PITR
# setup whose WAL lives only on the machine that died recovers nothing;
# the status endpoint says so in those words until this is turned on.
#
# WAL segments are many small immutable files, so they are mirrored with
# `aws s3 sync` / `rclone sync` rather than one upload per file. Immutable
# is what makes a size comparison sufficient there: a segment is written
# once, never edited.
#
# A target containing "://" is handled with the AWS CLI (any S3-compatible
# endpoint via AWS_ENDPOINT_URL); anything else is handled with rclone,
# whose remote names are configured outside this repository. Credentials
# are read from the environment by those tools — this script never sees,
# logs or records them.
#
# EXIT CODES
#   0  every eligible dump is off-site
#   1  something failed — and the failure IS recorded in the ledger, which
#      is what turns `/api/saas/backup/status` into `offsite: failing`
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

BACKUP_DIR="${BACKUP_DIR:-$REPO_ROOT/deploy/backups}"
LEDGER="${BACKUP_LEDGER_PATH:-$BACKUP_DIR/backups.jsonl}"
TARGET="${BACKUP_OFFSITE_TARGET:-}"
# How many of the most recent dumps to keep off-site in sync. The default
# of 0 means "every dump still present locally": local retention already
# decides the window, and a second, different window is a second thing to
# get wrong.
MAX_FILES="${BACKUP_OFFSITE_MAX_FILES:-0}"
INCLUDE_PITR="${BACKUP_OFFSITE_INCLUDE_PITR:-false}"
BASE_DIR="${BACKUP_BASE_DIR:-$BACKUP_DIR/base}"
WAL_ARCHIVE_DIR="${BACKUP_WAL_ARCHIVE_DIR:-$BACKUP_DIR/wal-archive}"
# Recorded in the ledger so /api/saas/backup/status can tell "the dumps
# are off-site" from "everything needed for a point-in-time recovery is
# off-site". Those are different promises.
SCOPE="dumps"
if [[ "$INCLUDE_PITR" == "true" || "$INCLUDE_PITR" == "1" ]]; then
    # Set BEFORE the preconditions run, so a failure record states what
    # this run was TRYING to protect rather than what it managed to.
    SCOPE="dumps+pitr"
fi
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

log() { echo "[$(date -u +%H:%M:%S)] $*"; }

record() {
    [[ "$DRY_RUN" -eq 1 ]] && return 0
    mkdir -p "$(dirname "$LEDGER")"
    printf '%s\n' "$1" >> "$LEDGER"
}

# The ledger records the KIND of destination ("s3", "rclone"), never the
# bucket, host, prefix or URL. The status endpoint reads this file, and a
# tenant-visible answer must never become a target list.
record_result() {
    local ok="$1" copied="$2" reason="${3//\"/\'}"
    record "{\"event\":\"offsite_synced\",\"at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"kind\":\"$KIND\",\"scope\":\"$SCOPE\",\"files\":$copied,\"ok\":$ok,\"detail\":\"$reason\"}"
}

die() {
    echo "FAIL: $*" >&2
    if [[ "$DRY_RUN" -eq 0 ]]; then
        record_result false "${COPIED:-0}" "$*"
    fi
    exit 1
}

# --- what are we talking to? -----------------------------------------------
KIND="none"
if [[ -z "$TARGET" ]]; then
    # Not configured is NOT an error: a single-host deployment is a valid
    # (documented) choice. It must simply never be reported as off-site.
    log "BACKUP_OFFSITE_TARGET is not set — no off-site copy is configured."
    log "The status endpoint will report local-only protection, which is the truth."
    exit 0
elif [[ "$TARGET" == *"://"* ]]; then
    KIND="s3"
else
    KIND="rclone"
fi

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

case "$KIND" in
    s3)     require "BACKUP_OFFSITE_TARGET looks like an S3 URL but the 'aws' CLI is not installed" command -v aws ;;
    rclone) require "BACKUP_OFFSITE_TARGET looks like an rclone remote but 'rclone' is not installed" command -v rclone ;;
esac
require "sha256sum is not available" command -v sha256sum

# --- which files? ----------------------------------------------------------
mapfile -t LOCAL < <(find "$BACKUP_DIR" -maxdepth 1 -type f \( -name '*.dump' -o -name '*.dump.age' \) -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | cut -d' ' -f2-)

if [[ "${#LOCAL[@]}" -eq 0 ]]; then
    # Nothing to copy is only good news if something was supposed to be
    # there. An empty backup directory with off-site configured means the
    # backup job never ran, so on a real run this is a FAILURE, not a
    # quiet success. A rehearsal only reports it.
    if [[ "$DRY_RUN" -eq 1 ]]; then
        log "DRY RUN: would FAIL — no local dump found in $BACKUP_DIR"
        log "DRY RUN: nothing to upload; the ledger was NOT touched"
        exit 0
    fi
    die "no local dump found in $BACKUP_DIR — the backup job has not produced anything to copy"
fi

if [[ "$MAX_FILES" -gt 0 && "${#LOCAL[@]}" -gt "$MAX_FILES" ]]; then
    LOCAL=("${LOCAL[@]:0:$MAX_FILES}")
fi

log "destination kind: $KIND · candidates: ${#LOCAL[@]}"

# --- one at a time ---------------------------------------------------------
LOCK="$BACKUP_DIR/.offsite.lock"
mkdir -p "$BACKUP_DIR"
exec 9>"$LOCK"
flock -n 9 || die "another off-site sync is already running (lock: $LOCK)"

COPIED=0
SKIPPED=0

remote_size() { # remote_size <basename> → bytes on stdout, empty when absent
    local name="$1"
    case "$KIND" in
        s3)
            local without=${TARGET#*://}
            local bucket=${without%%/*}
            local prefix=${without#"$bucket"}
            prefix=${prefix#/}
            local key="${prefix:+$prefix/}$name"
            aws s3api head-object --bucket "$bucket" --key "$key" \
                --query 'ContentLength' --output text 2>/dev/null || true
            ;;
        rclone)
            rclone size --json "$TARGET/$name" 2>/dev/null \
                | sed -n 's/.*"bytes":\([0-9]*\).*/\1/p' || true
            ;;
    esac
}

upload() { # upload <path> <basename> <sha256>
    local path="$1" name="$2" sha="$3"
    case "$KIND" in
        s3)
            # The checksum travels WITH the object as metadata so a later
            # drill can tell "this is the file we made" from "this is a
            # file with the right name".
            local sse=()
            [[ -n "${AWS_SERVER_SIDE_ENCRYPTION:-}" ]] && sse=(--sse "$AWS_SERVER_SIDE_ENCRYPTION")
            aws s3 cp "$path" "$TARGET/$name" \
                --only-show-errors \
                --metadata "sha256=$sha" \
                "${sse[@]+"${sse[@]}"}"
            ;;
        rclone)
            rclone copyto "$path" "$TARGET/$name" --quiet
            ;;
    esac
}

for path in "${LOCAL[@]}"; do
    name="$(basename "$path")"
    local_size="$(stat -c %s "$path")"
    sha="$(sha256sum "$path" | cut -d' ' -f1)"

    if [[ "$DRY_RUN" -eq 1 ]]; then
        log "DRY RUN: would upload $name ($local_size bytes) to the configured $KIND target"
        COPIED=$(( COPIED + 1 ))
        continue
    fi

    existing="$(remote_size "$name" || true)"
    if [[ -n "$existing" && "$existing" != "None" && "$existing" == "$local_size" ]]; then
        SKIPPED=$(( SKIPPED + 1 ))
        continue
    fi

    log "uploading $name ($local_size bytes)"
    upload "$path" "$name" "$sha" || die "upload of $name failed"

    # Verify AFTER the write. "The command exited 0" is not evidence the
    # bytes are there — a truncated multipart upload, a wrong prefix or a
    # bucket policy that silently drops writes all exit 0 somewhere.
    verified="$(remote_size "$name" || true)"
    [[ "$verified" == "$local_size" ]] \
        || die "post-upload verification failed for $name: remote reports '${verified:-absent}', local is $local_size bytes"

    COPIED=$(( COPIED + 1 ))
done

# --- the PITR artefacts ----------------------------------------------------
# Base backups and WAL segments. Mirrored as directories: a WAL archive
# is thousands of immutable files, and one process per file would take
# longer than the interval between segments on a busy database.
mirror_dir() { # mirror_dir <local dir> <remote subdirectory>
    local src="$1" sub="$2"
    [[ -d "$src" ]] || { log "no $sub directory at $src — skipping"; return 0; }
    local count
    count="$(find "$src" -type f ! -name '*.partial.*' | wc -l)"
    if [[ "$count" -eq 0 ]]; then
        log "$sub is empty — nothing to mirror"
        return 0
    fi
    if [[ "$DRY_RUN" -eq 1 ]]; then
        log "DRY RUN: would mirror $count file(s) from $sub to the configured $KIND target"
        COPIED=$(( COPIED + count ))
        return 0
    fi
    log "mirroring $sub ($count file(s))"
    case "$KIND" in
        s3)
            # --size-only is correct here and ONLY here: WAL segments and
            # base tarballs are written once and never modified, so a
            # size match is a content match. Never reuse this flag for
            # files that can be rewritten in place.
            aws s3 sync "$src" "$TARGET/$sub" \
                --only-show-errors --size-only \
                --exclude '*.partial.*' --exclude '.state/*' \
                || die "mirroring $sub failed"
            ;;
        rclone)
            rclone sync "$src" "$TARGET/$sub" --quiet \
                --size-only --exclude '*.partial.*' --exclude '.state/**' \
                || die "mirroring $sub failed"
            ;;
    esac
    COPIED=$(( COPIED + count ))
}

if [[ "$SCOPE" == "dumps+pitr" ]]; then
    mirror_dir "$BASE_DIR" "base"
    mirror_dir "$WAL_ARCHIVE_DIR" "wal-archive"
else
    # Said once, plainly, on every run: a reader of this log should not
    # have to infer it from the absence of output.
    if [[ -d "$WAL_ARCHIVE_DIR" ]] && [[ -n "$(find "$WAL_ARCHIVE_DIR" -maxdepth 1 -type f -name '[0-9A-F]*' -print -quit 2>/dev/null)" ]]; then
        log "WARNING: this host archives WAL, but BACKUP_OFFSITE_INCLUDE_PITR is not set."
        log "         Point-in-time recovery would NOT survive losing this host."
    fi
fi

if [[ "$DRY_RUN" -eq 1 ]]; then
    log "DRY RUN: would copy $COPIED file(s) (scope: $SCOPE); the ledger was NOT touched"
    [[ "$DRY_PROBLEMS" -gt 0 ]] && log "DRY RUN: $DRY_PROBLEMS precondition(s) unmet — a real run on THIS host would fail"
    exit 0
fi

record_result true "$COPIED" "uploaded $COPIED, already present $SKIPPED, scope $SCOPE"

log "off-site sync complete: uploaded $COPIED, already present $SKIPPED, scope $SCOPE"
echo
echo "An off-site copy nobody has restored FROM is still an assumption."
echo "The quarterly drill can pull straight from the off-site copy:"
echo "  ./scripts/verify-backup-restore.sh --from-offsite <backup_id>"
