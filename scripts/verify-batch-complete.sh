#!/usr/bin/env bash
# verify-batch-complete.sh — completeness gate for the integration-cleanup
# batch.
#
# FILES-MANIFEST.txt is the delivered present-file tree. Each line is:
#
#   <repo-relative path>\t<newline count>\t<sha256>
#
# This verifier fails when a manifest path is unsafe, duplicated, missing,
# empty, shorter than its category minimum, or different from the recorded
# line count/hash. It also parses the restored workflows and generated JSON,
# syntax-checks the new shell entry points, scans the delivered text for
# placeholder-style forbidden patterns, and runs the fast static gates that
# this cleanup batch is specifically responsible for keeping green.
#
# It does not replace the full Cargo/PostgreSQL/frontend acceptance run.
# Those commands are printed by APPLY.sh and recorded in the batch report.
# What this script proves is narrower and mechanical: the package contains
# complete, internally consistent files and the cleanup-owned static gates
# still pass on the exact tree being delivered.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MANIFEST="FILES-MANIFEST.txt"
EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
fail=0

note() {
    echo "[verify-batch-complete] $*"
}

error() {
    echo "[verify-batch-complete] FAIL — $*" >&2
    fail=1
}

if [[ ! -f "$MANIFEST" ]]; then
    error "$MANIFEST is missing"
    exit 1
fi
if [[ ! -f BASE_COMMIT.txt ]]; then
    error "BASE_COMMIT.txt is missing"
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if [[ "$BASE" != "$EXPECTED_BASE" ]]; then
    error "BASE_COMMIT.txt contains $BASE, expected $EXPECTED_BASE"
fi

# Existing scripts included only to record/apply their executable-mode fix.
# Their bytes are proved identical to the base by the ownership gate, so a
# generic 40-line minimum would be the wrong completeness rule for a valid
# pre-existing six-line wrapper. Every other category has a real minimum.
is_mode_only_script() {
    case "$1" in
        scripts/apply-batch01-deletes.sh) return 0 ;;
        scripts/bootstrap-config.sh) return 0 ;;
        scripts/check-batch02-ownership.sh) return 0 ;;
        scripts/check-batch03-ownership.sh) return 0 ;;
        scripts/check-batch04-ownership.sh) return 0 ;;
        scripts/check-rls-coverage.sh) return 0 ;;
        scripts/cleanup-tracked-junk.sh) return 0 ;;
        scripts/run-dataset-capture.sh) return 0 ;;
        scripts/telegram-trade-smoke.sh) return 0 ;;
        scripts/untrack-batch01-junk.sh) return 0 ;;
        scripts/verify-raydium-idl.sh) return 0 ;;
        tests/release/evidence_truth.sh) return 0 ;;
        *) return 1 ;;
    esac
}

# Existing documents whose only changes are generated stat-marker values.
# Ownership proves the non-marker bytes are identical to the base; several
# complete matrix documents are legitimately shorter than 40 lines because
# each table row is one very long line.
is_marker_only_document() {
    case "$1" in
        README.md) return 0 ;;
        docs/API-COMPATIBILITY-MATRIX.md) return 0 ;;
        docs/ARCHITECTURE-OVERVIEW.md) return 0 ;;
        docs/CAPABILITY-MATRIX.md) return 0 ;;
        docs/DELIVERY-MANIFEST.md) return 0 ;;
        docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md) return 0 ;;
        docs/EVIDENCE-INDEX.md) return 0 ;;
        docs/FEATURE-TRACEABILITY.md) return 0 ;;
        docs/IP-OWNERSHIP-REGISTER.md) return 0 ;;
        docs/OPERATIONS-RUNBOOK.md) return 0 ;;
        docs/PENETRATION-TEST-READINESS.md) return 0 ;;
        docs/PRODUCTION-READINESS-MATRIX.md) return 0 ;;
        docs/RELEASE-NOTES-0.1.0.md) return 0 ;;
        docs/SECURITY-CONTROLS-MATRIX.md) return 0 ;;
        docs/SECURITY-THREAT-MODEL.md) return 0 ;;
        docs/SELLER-FACT-SHEET.md) return 0 ;;
        docs/SELLING-LISTING-SOURCE.md) return 0 ;;
        docs/TECHNICAL-FACT-SHEET.md) return 0 ;;
        docs/TRANSACTION-READINESS-REPORT.md) return 0 ;;
        *) return 1 ;;
    esac
}

minimum_lines_for() {
    local path="$1"
    if is_mode_only_script "$path" || is_marker_only_document "$path"; then
        echo 1
        return
    fi
    case "$path" in
        docs/STATS.md) echo 20 ;;
        .github/workflows/*.yml) echo 100 ;;
        APPLY.sh) echo 150 ;;
        scripts/check-cleanup-ownership.sh) echo 250 ;;
        scripts/verify-batch-complete.sh) echo 250 ;;
        crates/server/tests/identity_plane_sql_contract.rs) echo 200 ;;
        *) echo 40 ;;
    esac
}

declare -A seen_paths=()
manifest_entries=0

note "checking manifest entries, line counts, hashes, and minimum sizes"
while IFS=$'\t' read -r path expected_lines expected_sha extra; do
    # A final empty line is not an entry. Any other malformed row is.
    if [[ -z "${path:-}" && -z "${expected_lines:-}" && -z "${expected_sha:-}" && -z "${extra:-}" ]]; then
        continue
    fi
    manifest_entries=$((manifest_entries + 1))

    if [[ -n "${extra:-}" ]]; then
        error "manifest entry has too many fields: $path"
        continue
    fi
    if [[ -z "${path:-}" || -z "${expected_lines:-}" || -z "${expected_sha:-}" ]]; then
        error "manifest entry is incomplete at row $manifest_entries"
        continue
    fi
    case "$path" in
        /*|../*|*/../*|*//*)
            error "unsafe manifest path: $path"
            continue
            ;;
    esac
    if [[ "$path" == "$MANIFEST" ]]; then
        error "FILES-MANIFEST.txt must not contain its own self-referential hash"
        continue
    fi
    if [[ -n "${seen_paths[$path]:-}" ]]; then
        error "duplicate manifest path: $path"
        continue
    fi
    seen_paths["$path"]=1

    if ! [[ "$expected_lines" =~ ^[0-9]+$ ]]; then
        error "manifest line count is not numeric for $path: $expected_lines"
        continue
    fi
    if ! [[ "$expected_sha" =~ ^[0-9a-f]{64}$ ]]; then
        error "manifest sha256 is malformed for $path: $expected_sha"
        continue
    fi
    if [[ ! -f "$path" ]]; then
        error "manifest file is missing: $path"
        continue
    fi
    if [[ ! -s "$path" ]]; then
        error "manifest file is empty: $path"
        continue
    fi

    actual_lines="$(wc -l < "$path" | tr -d ' ')"
    if [[ "$actual_lines" != "$expected_lines" ]]; then
        error "$path line count is $actual_lines, manifest says $expected_lines"
    fi
    minimum="$(minimum_lines_for "$path")"
    if (( actual_lines < minimum )); then
        error "$path has $actual_lines lines, below its $minimum-line minimum"
    fi

    actual_sha="$(sha256sum "$path" | awk '{ print $1 }')"
    if [[ "$actual_sha" != "$expected_sha" ]]; then
        error "$path sha256 is $actual_sha, manifest says $expected_sha"
    fi
done < "$MANIFEST"

if (( manifest_entries < 48 )); then
    error "manifest has only $manifest_entries entries; the cleanup tree is unexpectedly small"
else
    note "$manifest_entries manifest entries checked"
fi

# Required paths are asserted independently of the manifest loop. If an
# entry were accidentally omitted from FILES-MANIFEST.txt, the loop above
# would have nothing to hash; this explicit list catches that omission.
required_paths=(
    .github/workflows/datasets.yml
    .github/workflows/telegram-trade.yml
    .github/workflows/venues.yml
    APPLY.sh
    BASE_COMMIT.txt
    README.md
    crates/server/src/saas/auth_flows.rs
    crates/server/src/saas/password_reset.rs
    crates/server/tests/batch6_transaction_readiness.rs
    crates/server/tests/identity_plane_sql_contract.rs
    docs/BUSINESS-MATRIX-2026.md
    docs/FORENSIC-SQL-RESEARCH-2026.md
    docs/STATS.md
    docs/gap-updates/cleanup-rows.md
    release-manifest.json
    scripts/check-cleanup-ownership.sh
    scripts/forensic-sql-scan.sh
    scripts/sync-stat-markers.sh
    scripts/verify-batch-complete.sh
)
for path in "${required_paths[@]}"; do
    if [[ "$path" != "BASE_COMMIT.txt" && -z "${seen_paths[$path]:-}" ]]; then
        error "required path is absent from FILES-MANIFEST.txt: $path"
    fi
    if [[ ! -s "$path" ]]; then
        error "required path is missing or empty: $path"
    fi
done

note "syntax-checking batch shell entry points"
for path in \
    APPLY.sh \
    scripts/check-cleanup-ownership.sh \
    scripts/verify-batch-complete.sh \
    scripts/forensic-sql-scan.sh \
    scripts/sync-stat-markers.sh
do
    if ! bash -n "$path"; then
        error "shell syntax check failed: $path"
    fi
    if [[ ! -x "$path" ]]; then
        error "shell entry point is not executable on disk: $path"
    fi
done

note "parsing restored workflows and generated JSON"
if ! python3 - <<'PY'
import json
import sys
from pathlib import Path

try:
    import yaml
except ImportError as exc:
    print(f"PyYAML is required to parse the restored workflows: {exc}", file=sys.stderr)
    sys.exit(1)

expected_jobs = {
    ".github/workflows/datasets.yml": "datasets",
    ".github/workflows/venues.yml": "venues",
    ".github/workflows/telegram-trade.yml": "telegram-trade",
}
for relative, expected_job in expected_jobs.items():
    document = yaml.safe_load(Path(relative).read_text(encoding="utf-8"))
    jobs = document.get("jobs", {}) if isinstance(document, dict) else {}
    if expected_job not in jobs:
        print(f"{relative}: expected job {expected_job!r}, found {sorted(jobs)!r}", file=sys.stderr)
        sys.exit(1)

manifest = json.loads(Path("release-manifest.json").read_text(encoding="utf-8"))
for key in ("version", "rust_files", "docs_files", "migrations", "test_count"):
    if key not in manifest:
        print(f"release-manifest.json is missing required key {key!r}", file=sys.stderr)
        sys.exit(1)
PY
then
    error "workflow or release-manifest parsing failed"
fi

# Placeholder-pattern scan. Tokens are concatenated so this verifier does
# not match its own pattern definitions. The scan covers text whose content
# this batch authored or regenerated, except generated JSON. It does not
# re-litigate unchanged legacy vocabulary in mode-only scripts, marker-only
# documents, or the two pre-existing server source files; ownership proves
# those contents/marker boundaries, and the exact SQL changes are guarded
# by identity_plane_sql_contract.rs and the forensic scanner below.
note "scanning delivered text for placeholder patterns"
pattern_todo_bang='to''do!''('
pattern_unimplemented='unimpl''emented!''('
pattern_todo='TO''DO'
pattern_fixme='FIX''ME'
pattern_xxx='X''XX'
pattern_ignore='#''[ignore]'
pattern_skip='.''skip''('
pattern_or_true='|''| tru''e'
for path in "${!seen_paths[@]}"; do
    if is_mode_only_script "$path" || is_marker_only_document "$path"; then
        continue
    fi
    case "$path" in
        release-manifest.json|docs/archive/manifest-history-2026-10.json)
            continue
            ;;
        crates/server/src/saas/auth_flows.rs|crates/server/src/saas/password_reset.rs)
            continue
            ;;
    esac
    for pattern in \
        "$pattern_todo_bang" \
        "$pattern_unimplemented" \
        "$pattern_todo" \
        "$pattern_fixme" \
        "$pattern_xxx" \
        "$pattern_ignore" \
        "$pattern_skip" \
        "$pattern_or_true"
    do
        if grep -Fq -- "$pattern" "$path"; then
            error "forbidden placeholder pattern found in $path: $pattern"
        fi
    done
done

# The new Rust contract has a stricter rule than the legacy tree: its test
# helpers use explicit panic messages on unreadable fixtures instead of
# unwrap/expect shortcuts, and every test makes assertions.
new_rust_test="crates/server/tests/identity_plane_sql_contract.rs"
if grep -Fq '.un''wrap()' "$new_rust_test"; then
    error "new identity contract test contains an unwrap shortcut"
fi
if grep -Fq '.ex''pect(' "$new_rust_test"; then
    error "new identity contract test contains an expect shortcut"
fi
if ! assertion_count="$(grep -Ec 'assert!|assert_eq!' "$new_rust_test")"; then
    assertion_count=0
fi
if (( assertion_count < 20 )); then
    error "new identity contract test has only $assertion_count assertions"
fi

# Fast gates owned by this cleanup. Each is run directly and its exit code
# is honored; there is no output-pattern substitute for a failing process.
run_gate() {
    local label="$1"
    shift
    note "running $label"
    if ! "$@"; then
        error "$label failed"
    fi
}

run_gate "tracked script modes" bash scripts/verify-script-modes.sh
run_gate "forensic SQL sweep" bash scripts/forensic-sql-scan.sh
run_gate "generated statistics" bash tests/release/stats_current.sh
run_gate "release manifest currency" bash tests/release/manifest_current.sh
run_gate "business matrix completeness" bash tests/business/business-matrix-completeness.sh
run_gate "integration-cleanup ownership" bash scripts/check-cleanup-ownership.sh

if (( fail )); then
    echo "[verify-batch-complete] FAIL" >&2
    exit 1
fi

note "PASS — manifest, complete-file minimums, syntax, parsing, placeholder scan, and cleanup static gates are green"
