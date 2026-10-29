#!/usr/bin/env bash
# forensic_sql_extractor.sh — prove the forensic SQL scanner's EXTRACTOR
# rather than its verdict.
#
# scripts/forensic-sql-scan.sh can only classify the statements it can read.
# A statement the extractor misses is not "safe": it is unseen. This suite
# therefore sweeps each fixture in tests/release/forensic_sql_fixtures on its
# own and asserts the exact statement count, and it asserts that the one
# fixture written to break the tenant-isolation contract
# (tenant_missing.rs) makes the gate FAIL.
#
# Checks:
#   1. multiline_format.rs — 2 statements, both read, no class-4 (exit 0)
#   2. raw_string.rs       — 2 statements, both read (inline + `const`), exit 0
#   3. query_macros.rs     — 3 statements (query!/query_as!/query_scalar!), exit 0
#   4. tenant_missing.rs   — 2 statements, 1 class-4, gate exits 1
#   5. the whole fixture tree — exactly one class-4, from tenant_missing.rs
#   6. the real crates/ tree — extraction coverage inside the 2% budget
#
# Exit: 0 = all assertions held; 1 = at least one failed.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCAN="$ROOT/scripts/forensic-sql-scan.sh"
FIXTURES="$ROOT/tests/release/forensic_sql_fixtures"

fail=0
pass=0

ok()   { pass=$((pass + 1)); printf 'PASS  %s\n' "$1"; }
bad()  { fail=$((fail + 1)); printf 'FAIL  %s\n' "$1" >&2; }

if [[ ! -x "$SCAN" ]]; then
    bad "scripts/forensic-sql-scan.sh is missing or not executable"
    exit 1
fi
if [[ ! -d "$FIXTURES" ]]; then
    bad "fixture directory missing: ${FIXTURES#$ROOT/}"
    exit 1
fi

# Run the scanner against a tree and echo "<exit-code>\n<output>".
sweep() {
    local target="$1"
    local out rc
    out="$(bash "$SCAN" "$target" 2>/dev/null)"
    rc=$?
    printf '%s\n%s\n' "$rc" "$out"
}

field() {
    # field "<label>" -> the first number that follows the label on its line
    printf '%s\n' "$2" | sed -n "s/.*$1[^0-9]*\([0-9][0-9]*\).*/\1/p" | head -1
}

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# ---------------------------------------------------------------------------
# 1-4. One fixture at a time: exact statement counts and the expected verdict.
# ---------------------------------------------------------------------------
# name|expected statements|expected exit code|expected class-4 count
CASES=(
    "multiline_format.rs|2|0|0"
    "raw_string.rs|2|0|0"
    "query_macros.rs|3|0|0"
    "tenant_missing.rs|2|1|1"
)

for case in "${CASES[@]}"; do
    name="${case%%|*}"
    rest="${case#*|}"
    want_statements="${rest%%|*}"
    rest="${rest#*|}"
    want_exit="${rest%%|*}"
    want_class4="${rest##*|}"

    src="$FIXTURES/$name"
    if [[ ! -f "$src" ]]; then
        bad "$name: fixture is missing"
        continue
    fi

    # Sweep the fixture alone: one file in, one file's worth of findings out.
    single="$WORKDIR/single-$name"
    mkdir -p "$single"
    cp "$src" "$single/"

    result="$(sweep "$single")"
    rc="${result%%$'\n'*}"
    out="${result#*$'\n'}"

    got_statements="$(field "statements extracted:" "$out")"
    got_sites="$(field "call sites:" "$out")"
    got_unparsed="$(field "unparsed" "$out")"
    got_class4="$(field "class 4 (MISSING-TENANT-ENFORCEMENT):" "$out")"
    got_class4="${got_class4:-0}"

    if [[ "$rc" != "$want_exit" ]]; then
        bad "$name: scanner exited $rc, expected $want_exit"
    else
        ok "$name: scanner exited $rc as expected"
    fi

    if [[ "$got_statements" != "$want_statements" ]]; then
        bad "$name: extracted $got_statements statements, expected $want_statements"
    else
        ok "$name: extracted exactly $want_statements statements"
    fi

    if [[ -n "$got_sites" && "$got_statements" != "$got_sites" ]]; then
        bad "$name: $got_sites call sites but only $got_statements extracted"
    else
        ok "$name: every call site was extracted"
    fi

    if [[ "$got_unparsed" != "0" ]]; then
        bad "$name: $got_unparsed unparsed call site(s)"
    else
        ok "$name: zero unparsed call sites"
    fi

    if [[ "$got_class4" != "$want_class4" ]]; then
        bad "$name: $got_class4 class-4 finding(s), expected $want_class4"
    else
        ok "$name: $want_class4 class-4 finding(s) as expected"
    fi

    # The class-4 finding, when there is one, must point at this fixture.
    if [[ "$want_class4" != "0" ]]; then
        if printf '%s\n' "$out" | grep -q "^  ${name}:"; then
            ok "$name: class-4 finding is attributed to ${name}"
        else
            bad "$name: no class-4 finding attributed to ${name}"
        fi
    fi
done

# ---------------------------------------------------------------------------
# 5. The whole fixture tree: exactly one class-4, from tenant_missing.rs.
# ---------------------------------------------------------------------------
result="$(sweep "$FIXTURES")"
rc="${result%%$'\n'*}"
out="${result#*$'\n'}"
got_class4="$(field "class 4 (MISSING-TENANT-ENFORCEMENT):" "$out")"
got_class4="${got_class4:-0}"

if [[ "$rc" != "1" ]]; then
    bad "fixture tree: scanner exited $rc, expected 1 (tenant_missing.rs must fail the gate)"
else
    ok "fixture tree: scanner exits 1 because tenant_missing.rs breaks the contract"
fi
if [[ "$got_class4" != "1" ]]; then
    bad "fixture tree: $got_class4 class-4 finding(s), expected exactly 1"
else
    ok "fixture tree: exactly one class-4 finding"
fi
if printf '%s\n' "$out" | grep -q "^  tenant_missing.rs:"; then
    ok "fixture tree: the class-4 finding is tenant_missing.rs"
else
    bad "fixture tree: the class-4 finding is not tenant_missing.rs"
fi

# ---------------------------------------------------------------------------
# 6. The real tree: extraction coverage must stay inside the 2% budget.
# ---------------------------------------------------------------------------
result="$(sweep "$ROOT")"
rc="${result%%$'\n'*}"
out="${result#*$'\n'}"
got_sites="$(field "call sites:" "$out")"
got_unparsed="$(field "unparsed" "$out")"
got_class4="$(field "class 4 (MISSING-TENANT-ENFORCEMENT):" "$out")"
got_class4="${got_class4:-0}"

if [[ -z "$got_sites" || "$got_sites" -lt 100 ]]; then
    bad "crates/ tree: only $got_sites call sites found — the sweep is not seeing the codebase"
else
    ok "crates/ tree: $got_sites sqlx::query call sites swept"
fi

if [[ -z "$got_unparsed" ]]; then
    bad "crates/ tree: no extraction-coverage line in the report"
else
    # Integer-safe percentage: unparsed * 100 / sites, compared to 2.
    pct=$(( got_unparsed * 100 / got_sites ))
    if (( pct > 2 )); then
        bad "crates/ tree: extraction failures ${got_unparsed}/${got_sites} (${pct}%) exceed the 2% budget"
    else
        ok "crates/ tree: extraction failures ${got_unparsed}/${got_sites} (${pct}%) inside the 2% budget"
    fi
fi

if [[ "$got_class4" != "0" ]]; then
    bad "crates/ tree: $got_class4 class-4 finding(s)"
else
    ok "crates/ tree: zero class-4 findings"
fi

# The scanner must exit 0 clean for the real tree, not merely fail quietly.
if [[ "$rc" != "0" ]]; then
    bad "crates/ tree: scanner exited $rc, expected 0"
else
    ok "crates/ tree: scanner exits 0"
fi

echo "----------------------------------------"
echo "forensic_sql_extractor: ${pass} PASS / ${fail} FAIL"
[[ "$fail" -eq 0 ]] || exit 1
exit 0
