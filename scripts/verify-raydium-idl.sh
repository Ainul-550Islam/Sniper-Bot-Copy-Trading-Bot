#!/usr/bin/env bash
# Verify the vendored Raydium IDLs against their pinned upstream provenance.
#
# This script is **offline and deterministic**: it does not clone anything and
# does not touch the network. It recomputes the sha256 of every vendored IDL
# and compares it with the value recorded in `crates/solana-kit/idl/SOURCES.md`,
# and it cross-checks the program ids and the Anchor sighash discriminators
# recorded in `crates/solana-kit/src/consts.rs` against the IDL JSON itself.
#
# Exits non-zero on any drift.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

IDL_DIR="crates/solana-kit/idl"
SOURCES="$IDL_DIR/SOURCES.md"
CONSTS="crates/solana-kit/src/consts.rs"

fail=0
note() { printf 'verify-raydium-idl: %s\n' "$*"; }
fail() { printf 'verify-raydium-idl: FAIL: %s\n' "$*" >&2; fail=1; }

for f in "$IDL_DIR/raydium_cp_swap.json" "$IDL_DIR/raydium_launchpad.json" "$SOURCES"; do
    if [[ ! -f "$f" ]]; then
        fail "missing $f"
    fi
done
(( fail == 0 )) || exit 1

sha_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "verify-raydium-idl: FAIL: no sha256sum or shasum available" >&2
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# 1. The recorded sha256 of every vendored IDL must still match.
# ---------------------------------------------------------------------------
# The table in SOURCES.md is the source of truth: parse it rather than
# hard-coding the hashes a second time.
check_sha() {
    local file="$1"
    local expected
    expected="$(awk -v want="$file" '
        $0 ~ "\\| `" want "` \\|" {
            if (match($0, /`[0-9a-f]{64}`/)) {
                print substr($0, RSTART + 1, 64)
                exit
            }
        }
    ' "$SOURCES")"
    if [[ -z "$expected" ]]; then
        fail "no sha256 recorded in $SOURCES for $file"
        return
    fi
    local actual
    actual="$(sha_of "$IDL_DIR/$file")"
    if [[ "$actual" != "$expected" ]]; then
        fail "sha256 drift for $file: expected $expected, got $actual"
    else
        note "sha256 OK: $file ($actual)"
    fi
}

check_sha "raydium_cp_swap.json"
check_sha "raydium_launchpad.json"

# ---------------------------------------------------------------------------
# 2. The program ids in `consts.rs` must equal the IDLs' `address` fields.
# ---------------------------------------------------------------------------
# `consts.rs` declares each program id as a base58 literal:
#   pub static RAYDIUM_CPMM: Lazy<Pubkey> =
#       Lazy::new(|| pk("CPMMoo8..."));
read_const_program_id() {
    python3 - "$CONSTS" "$1" <<'PYEOF'
import re
import sys

path, name = sys.argv[1], sys.argv[2]
src = open(path, "r", encoding="utf-8").read()
pattern = (
    r"pub static\s+" + re.escape(name) + r"\s*:\s*Lazy<Pubkey>\s*=\s*"
    r"Lazy::new\(\|\|\s*pk\(\s*\"([1-9A-HJ-NP-Za-km-z]+)\"\s*\)"
)
m = re.search(pattern, src)
print(m.group(1) if m else "")
PYEOF
}

read_idl_address() {
    python3 - "$IDL_DIR/$1" <<'PYEOF'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    print(json.load(fh)["address"])
PYEOF
}

check_program_id() {
    local file="$1"
    local const_name="$2"
    local idl_address actual
    idl_address="$(read_idl_address "$file")"
    actual="$(read_const_program_id "$const_name")"
    if [[ -z "$actual" ]]; then
        fail "could not read $const_name from $CONSTS"
    elif [[ "$actual" != "$idl_address" ]]; then
        fail "program id mismatch: $const_name is $actual but the IDL address is $idl_address"
    else
        note "program id OK: $const_name == $idl_address"
    fi
}

check_program_id "raydium_cp_swap.json" "RAYDIUM_CPMM"
check_program_id "raydium_launchpad.json" "RAYDIUM_LAUNCHLAB"

# ---------------------------------------------------------------------------
# 3. The Anchor sighash convention must hold for every discriminator in the
#    vendored IDLs. This is the same check the Rust tests make, expressed here
#    so a reviewer can run it without a Rust toolchain.
# ---------------------------------------------------------------------------
if ! python3 - "$IDL_DIR" <<'PYEOF'
import hashlib
import json
import sys

idl_dir = sys.argv[1]
problems = []


def sighash(prefix, name):
    return hashlib.sha256(f"{prefix}:{name}".encode()).digest()[:8]


def check(path):
    with open(path, "r", encoding="utf-8") as fh:
        idl = json.load(fh)
    count = 0
    for section, prefix in (("instructions", "global"), ("accounts", "account")):
        for entry in idl.get(section, []):
            name = entry["name"]
            disc = bytes(entry["discriminator"])
            count += 1
            expected = sighash(prefix, name)
            if disc != expected:
                problems.append(
                    f"{path}: {section}/{name} discriminator {disc.hex()} != "
                    f"sha256('{prefix}:{name}')[..8] = {expected.hex()}"
                )
    for entry in idl.get("events", []):
        name = entry["name"]
        disc = bytes(entry["discriminator"])
        count += 1
        expected = sighash("event", name)
        if disc != expected:
            problems.append(
                f"{path}: events/{name} discriminator {disc.hex()} != "
                f"sha256('event:{name}')[..8] = {expected.hex()}"
            )
    return count


total = 0
total += check(f"{idl_dir}/raydium_cp_swap.json")
total += check(f"{idl_dir}/raydium_launchpad.json")

if problems:
    for p in problems:
        print(f"verify-raydium-idl: FAIL: {p}", file=sys.stderr)
    sys.exit(1)

print(
    f"verify-raydium-idl: OK — {total} IDL discriminators follow the Anchor sighash convention"
)
PYEOF
then
    fail "the Anchor sighash / IDL discriminator cross-check failed"
fi

# ---------------------------------------------------------------------------
# 4. Every `RAYDIUM_*_DISC_*` constant in `consts.rs` must be reproducible from
#    the Anchor sighash convention.
# ---------------------------------------------------------------------------
if ! python3 - "$CONSTS" <<'PYEOF'
import hashlib
import re
import sys

path = sys.argv[1]
src = open(path, "r", encoding="utf-8").read()

pattern = re.compile(
    r"pub const (RAYDIUM_[A-Z0-9_]*DISC[A-Z0-9_]*)\s*:\s*\[u8;\s*8\]\s*=\s*\[([0-9,\s]*)\];"
)

problems = []
checked = 0
for m in pattern.finditer(src):
    name = m.group(1)
    nums = [int(x) for x in m.group(2).split(",") if x.strip()]
    if len(nums) != 8:
        problems.append(f"{name}: expected 8 bytes, found {len(nums)}")
        continue
    actual = bytes(nums)
    # Recover the Anchor name from the constant name. The convention in this
    # crate is `RAYDIUM_<VENUE>_DISC_<ANCHOR_NAME>` for instructions,
    # `RAYDIUM_<VENUE>_EV_<EVENT_NAME>` for events and
    # `RAYDIUM_<VENUE>_ACC_DISC_<ACCOUNT_NAME>` for accounts. The Anchor name
    # is CamelCase, so the snake_case constant cannot be reversed exactly;
    # instead the constant is checked against the sighash of every candidate
    # spelling derived from the constant name.
    body = name[len("RAYDIUM_") :]
    for venue in ("CPMM_", "LAUNCHLAB_"):
        if body.startswith(venue):
            body = body[len(venue) :]
            break
    if body.startswith("ACC_DISC_"):
        body = body[len("ACC_DISC_") :]
        prefix = "account"
    elif body.startswith("EV_"):
        body = body[len("EV_") :]
        prefix = "event"
    elif body.startswith("DISC_"):
        body = body[len("DISC_") :]
        prefix = "global"
    else:
        continue

    candidates = {body.lower()}
    # `trade` -> `Trade`, `pool_state` -> `PoolState`.
    candidates.add("".join(w.capitalize() for w in body.lower().split("_")))

    matched = False
    for cand in candidates:
        if hashlib.sha256(f"{prefix}:{cand}".encode()).digest()[:8] == actual:
            matched = True
            break
    if not matched:
        problems.append(
            f"{name}: {actual.hex()} does not match sha256(\"{prefix}:<name>\")[..8] "
            f"for any spelling derived from the constant name"
        )
    else:
        checked += 1

if problems:
    for p in problems:
        print(f"verify-raydium-idl: FAIL: {p}", file=sys.stderr)
    sys.exit(1)

print(f"verify-raydium-idl: OK — {checked} discriminator constants follow the sighash convention")
PYEOF
then
    fail "a discriminator constant in consts.rs does not follow the sighash convention"
fi

if (( fail )); then
    exit 1
fi

note "PASS — the vendored Raydium IDLs match their pinned upstream provenance"
