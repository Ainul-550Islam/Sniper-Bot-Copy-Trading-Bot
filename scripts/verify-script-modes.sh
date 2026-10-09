#!/usr/bin/env bash
# verify-script-modes.sh — every shell script must be executable.
#
# Two modes:
#   * Inside a git repository (no arguments): every TRACKED *.sh must have
#     index mode 100755 (git ls-files -s). A tracked script committed as
#     100644 breaks every checkout that expects to run it directly.
#   * With directory arguments (or outside a git repository): every *.sh
#     under those directories must have the owner-execute bit on disk
#     (find ... ! -perm -u+x). This is how the release-package staging
#     tree is checked post-build.
#
# Offenders are printed one per line; exit 1 if any, exit 0 when clean.
#
# Exclusions: intentionally sourced-and-never-executed scripts may be
# listed below by EXACT PATH with a reason (never by pattern).
set -euo pipefail

# Explicit-path exclusions only. Currently empty: every tracked *.sh in this
# repository is executed directly (verified for P0-C TASK 6).
EXCLUDES=""

is_excluded() {
    case " $EXCLUDES " in
        *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
}

offenders=0

if [ "$#" -eq 0 ] && git rev-parse --git-dir >/dev/null 2>&1; then
    # Tracked-script check: index mode must be 100755.
    while read -r mode _hash _stage path; do
        is_excluded "$path" && continue
        if [ "$mode" != "100755" ]; then
            echo "MODE OFFENDER (tracked, index mode $mode): $path"
            offenders=$((offenders + 1))
        fi
    done < <(git ls-files -s -- '*.sh')
    if [ "$offenders" -eq 0 ]; then
        echo "verify-script-modes: OK — every tracked *.sh has index mode 100755."
    else
        echo "verify-script-modes: FAIL — $offenders tracked *.sh without mode 100755." >&2
        echo "Fix: chmod +x <path> && git update-index --chmod=+x <path>" >&2
        exit 1
    fi
    exit 0
fi

# Directory check (also used outside git).
if [ "$#" -eq 0 ]; then
    set -- .
fi
for dir in "$@"; do
    [ -d "$dir" ] || { echo "verify-script-modes: not a directory: $dir" >&2; exit 2; }
    while IFS= read -r f; do
        is_excluded "$f" && continue
        echo "MODE OFFENDER (not executable): $f"
        offenders=$((offenders + 1))
    done < <(find "$dir" -name '*.sh' ! -perm -u+x)
done

if [ "$offenders" -eq 0 ]; then
    echo "verify-script-modes: OK — every *.sh under the given tree is executable."
else
    echo "verify-script-modes: FAIL — $offenders non-executable *.sh file(s)." >&2
    exit 1
fi
