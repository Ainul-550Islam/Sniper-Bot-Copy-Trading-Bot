#!/usr/bin/env bash
# BATCH 04 — fail if this batch changed any path outside its ownership contract.
#
# The diff starts at the exact commit recorded in BASE_COMMIT.txt (the commit
# Batch 04 branched from); untracked files are included so a forgotten new file
# cannot evade the gate. Two shared files are allowed only in their narrowest
# possible form, and both forms are proved here rather than asserted:
#
#   * `Cargo.lock`          — allowed, but every changed line must sit inside
#                             the `[[package]] name = "module-telegram"`
#                             stanza (no new crates, no version churn);
#   * `crates/server/src/lib.rs`
#                           — allowed, but the ONLY change may be the single
#                             `pub mod telegram_trade_store_pg;` line.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ ! -f BASE_COMMIT.txt ]]; then
    echo "check-batch04-ownership: BASE_COMMIT.txt is missing" >&2
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if ! [[ "$BASE" =~ ^[0-9a-f]{40}$ ]] \
    || ! git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
    echo "check-batch04-ownership: invalid or unavailable base commit: $BASE" >&2
    exit 1
fi

is_allowed() {
    case "$1" in
        # Batch 04 bookkeeping.
        BASE_COMMIT.txt|APPLY.sh|SHA256SUMS) return 0 ;;
        # Allowed only inside the module-telegram stanza (checked below).
        Cargo.lock) return 0 ;;
        # Migrations this batch owns (contiguous, nothing else in 00xx).
        crates/core/migrations/0066_telegram_trade_consents.sql) return 0 ;;
        crates/core/migrations/0067_telegram_trade_commands.sql) return 0 ;;
        # The module itself.
        crates/module-telegram/Cargo.toml) return 0 ;;
        crates/module-telegram/src/*.rs) return 0 ;;
        crates/module-telegram/src/commands/*.rs) return 0 ;;
        crates/module-telegram/src/consent/*.rs) return 0 ;;
        crates/module-telegram/tests/*.rs) return 0 ;;
        crates/module-telegram/tests/support/*.rs) return 0 ;;
        # The optional durable store in the server crate (one module + one
        # `pub mod` line, checked separately below).
        crates/server/src/telegram_trade_store_pg.rs) return 0 ;;
        crates/server/src/lib.rs) return 0 ;;
        crates/server/tests/telegram_trade_pg.rs) return 0 ;;
        crates/server/tests/telegram_trade_migrations.rs) return 0 ;;
        # Batch 04 gates, CI job and docs.
        scripts/check-batch04-ownership.sh) return 0 ;;
        scripts/telegram-trade-smoke.sh) return 0 ;;
        .github/workflows/telegram-trade.yml) return 0 ;;
        .github/workflows/ci.yml) return 1 ;;
        docs/TELEGRAM-TRADING.md) return 0 ;;
        docs/gap-updates/batch-04-rows.md) return 0 ;;
        docs/gap-updates/batch-04-wiring.md) return 0 ;;
        # The pre-move location: tolerated only so an in-flight branch that has
        # not rebased onto the move still passes; it must not exist.
        docs/batch-04-wiring.md) return 1 ;;
        *) return 1 ;;
    esac
}

mapfile -t changed < <(
    {
        git diff --name-only "$BASE" --
        git ls-files --others --exclude-standard
    } | sed '/^[[:space:]]*$/d' | sort -u
)

bad=0
for path in "${changed[@]}"; do
    if ! is_allowed "$path"; then
        echo "check-batch04-ownership: FORBIDDEN changed path: $path" >&2
        bad=1
    fi
done

# ---- Cargo.lock: changes only inside the module-telegram package stanza ------
if printf '%s\n' "${changed[@]}" | grep -qx 'Cargo.lock'; then
    outside="$(
        git diff "$BASE" -- Cargo.lock | awk '
            /^(---|\+\+\+) / { next }
            {
                prefix = substr($0, 1, 1)
                line = substr($0, 2)
                if (line ~ /^\[\[package\]\]/) { pkg = ""; next }
                if (line ~ /^name = "/) {
                    pkg = line
                    sub(/^name = "/, "", pkg)
                    sub(/"$/, "", pkg)
                    next
                }
                if ((prefix == "+" || prefix == "-") && pkg != "" && pkg != "module-telegram") {
                    print $0
                }
            }
        '
    )"
    if [[ -n "$outside" ]]; then
        echo "check-batch04-ownership: Cargo.lock changed outside the module-telegram stanza:" >&2
        printf '%s\n' "$outside" >&2
        bad=1
    fi
fi

# ---- crates/server/src/lib.rs: exactly one added `pub mod` line -------------
if printf '%s\n' "${changed[@]}" | grep -qx 'crates/server/src/lib.rs'; then
    # `grep -c` exits 1 when nothing matches; read the count and let the
    # comparison below decide. The status is normalised explicitly so `set -e`
    # cannot abort the check on the legitimate "zero added lines" case.
    added="$(git diff "$BASE" -- crates/server/src/lib.rs | grep -cE '^\+[^+]')" || added=0
    removed="$(git diff "$BASE" -- crates/server/src/lib.rs | grep -cE '^-[^-]')" || removed=0
    if [[ "$added" != "1" || "$removed" != "0" ]]; then
        echo "check-batch04-ownership: crates/server/src/lib.rs must gain exactly one line" >&2
        echo "  (+$added / -$removed)" >&2
        bad=1
    fi
    if ! git diff "$BASE" -- crates/server/src/lib.rs | grep -qE '^\+pub mod telegram_trade_store_pg;$'; then
        echo "check-batch04-ownership: the added lib.rs line is not the expected module" >&2
        bad=1
    fi
fi

if (( bad )); then
    echo "check-batch04-ownership: FAIL" >&2
    exit 1
fi

# ---- the owned tree must be complete, not merely permitted -------------------
# Every file the batch is supposed to ship has to exist and be a real
# implementation: a present-but-empty file is how a "placeholder" slips through
# a review. Bookkeeping files are exempt (a commit hash is one line by nature).
TREE=(
    crates/core/migrations/0066_telegram_trade_consents.sql
    crates/core/migrations/0067_telegram_trade_commands.sql
    crates/module-telegram/Cargo.toml
    crates/module-telegram/src/lib.rs
    crates/module-telegram/src/commands.rs
    crates/module-telegram/src/trade_port.rs
    crates/module-telegram/src/consent/mod.rs
    crates/module-telegram/src/consent/model.rs
    crates/module-telegram/src/consent/policy.rs
    crates/module-telegram/src/consent/store.rs
    crates/module-telegram/src/commands/parse.rs
    crates/module-telegram/src/commands/nonce.rs
    crates/module-telegram/src/commands/pending.rs
    crates/module-telegram/src/commands/limits.rs
    crates/module-telegram/src/commands/audit.rs
    crates/module-telegram/src/commands/consent_cmd.rs
    crates/module-telegram/src/commands/buy.rs
    crates/module-telegram/src/commands/sell.rs
    crates/module-telegram/src/commands/confirm.rs
    crates/module-telegram/src/commands/reply.rs
    crates/module-telegram/tests/support/mod.rs
    crates/module-telegram/tests/parse_and_validation.rs
    crates/module-telegram/tests/consent_policy.rs
    crates/module-telegram/tests/nonce_and_pending.rs
    crates/module-telegram/tests/limits_and_rate.rs
    crates/module-telegram/tests/buy_sell_flow.rs
    crates/module-telegram/tests/audit_redaction.rs
    crates/module-telegram/tests/existing_commands_unchanged.rs
    crates/server/src/telegram_trade_store_pg.rs
    crates/server/tests/telegram_trade_pg.rs
    crates/server/tests/telegram_trade_migrations.rs
    scripts/check-batch04-ownership.sh
    scripts/telegram-trade-smoke.sh
    .github/workflows/telegram-trade.yml
    docs/TELEGRAM-TRADING.md
    docs/gap-updates/batch-04-rows.md
    docs/gap-updates/batch-04-wiring.md
)

# Bookkeeping: a one-line commit hash is correct, not incomplete.
is_bookkeeping() {
    case "$1" in
        BASE_COMMIT.txt|APPLY.sh|SHA256SUMS|Cargo.lock) return 0 ;;
        *) return 1 ;;
    esac
}

lines=0
for path in "${TREE[@]}"; do
    if [[ ! -f "$path" ]]; then
        echo "check-batch04-ownership: MISSING owned file: $path" >&2
        bad=1
        continue
    fi
    lines="$(wc -l < "$path" | tr -d ' ')"
    if (( lines < 40 )); then
        echo "check-batch04-ownership: $path has only $lines lines (need >= 40)" >&2
        bad=1
    fi
done

# Anything inside the owned directories that is not in the list above is a file
# nobody reviewed: fail on it too, so the tree cannot silently grow.
while IFS= read -r path; do
    [[ -z "$path" ]] && continue
    is_bookkeeping "$path" && continue
    found=0
    for known in "${TREE[@]}"; do
        [[ "$path" == "$known" ]] && found=1 && break
    done
    if (( ! found )); then
        echo "check-batch04-ownership: unlisted file inside the owned tree: $path" >&2
        bad=1
    fi
done < <(
    {
        git diff --name-only "$BASE" -- crates/module-telegram crates/server/src/telegram_trade_store_pg.rs \
            crates/server/tests/telegram_trade_pg.rs crates/server/tests/telegram_trade_migrations.rs \
            crates/core/migrations/0066_telegram_trade_consents.sql \
            crates/core/migrations/0067_telegram_trade_commands.sql scripts docs .github/workflows
        git ls-files --others --exclude-standard -- crates/module-telegram \
            crates/server/tests scripts docs .github/workflows
    } | sed '/^[[:space:]]*$/d' | sort -u
)

if (( bad )); then
    echo "check-batch04-ownership: FAIL" >&2
    exit 1
fi

echo "check-batch04-ownership: PASS — ${#changed[@]} changed file(s), all owned by Batch 04"
echo "check-batch04-ownership: PASS — ${#TREE[@]} owned files present, each >= 40 lines"
