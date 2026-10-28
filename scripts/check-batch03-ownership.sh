#!/usr/bin/env bash
# Fail if this batch changed any path outside its parallel-ownership contract.
#
# The diff always starts at the exact commit recorded when Batch 03 began;
# untracked files are included so a forgotten new file cannot evade the gate.
#
# Batch 03 owns exactly:
#   * the vendored Raydium IDLs and their provenance note,
#   * the two new Raydium venue modules inside `solana-kit`,
#   * the Raydium constants and the venue registry,
#   * the synthetic fixtures,
#   * the Raydium integration tests,
#   * this batch's scripts, workflow and docs.
#
# It owns **nothing** in `crates/module-sniper`, `crates/server`,
# `crates/core`, `crates/module-telegram`, `crates/module-copy`,
# `crates/module-polymarket`, `crates/core/migrations`, `apps`, `openapi`,
# `Cargo.lock`, `rust-toolchain.toml`, `config.toml*`, `.env.template`,
# `.github/workflows/ci.yml`, `docs/STATS.md`, `release-manifest.json`,
# `docs/KNOWN-LIMITATIONS.md`, `OWNER-TODO.md` or `docs/GAP-MISSING-FILES.md`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ ! -f BASE_COMMIT.txt ]]; then
    echo "check-batch03-ownership: BASE_COMMIT.txt is missing" >&2
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if ! [[ "$BASE" =~ ^[0-9a-f]{40}$ ]] \
    || ! git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
    echo "check-batch03-ownership: invalid or unavailable base commit: $BASE" >&2
    exit 1
fi

is_allowed() {
    case "$1" in
        BASE_COMMIT.txt|APPLY.sh|SHA256SUMS) return 0 ;;
        # Vendored official IDLs and their provenance.
        crates/solana-kit/idl/raydium_cp_swap.json) return 0 ;;
        crates/solana-kit/idl/raydium_launchpad.json) return 0 ;;
        crates/solana-kit/idl/SOURCES.md) return 0 ;;
        # Touched `solana-kit` sources.
        crates/solana-kit/src/consts.rs) return 0 ;;
        crates/solana-kit/src/venues/mod.rs) return 0 ;;
        # The two new venue modules. The pre-existing CPMM stub adapter
        # (`raydium_cpmm.rs`) is *replaced* by the `raydium_cpmm/` directory,
        # so its deletion is part of this batch.
        crates/solana-kit/src/venues/raydium_cpmm.rs) return 0 ;;
        crates/solana-kit/src/venues/raydium_cpmm/*) return 0 ;;
        crates/solana-kit/src/venues/raydium_launchlab/*) return 0 ;;
        # Synthetic fixtures.
        crates/solana-kit/fixtures/raydium/*) return 0 ;;
        # The integration tests.
        crates/solana-kit/tests/raydium_cpmm_*.rs) return 0 ;;
        crates/solana-kit/tests/raydium_launchlab_*.rs) return 0 ;;
        crates/solana-kit/tests/venue_registry_raydium.rs) return 0 ;;
        # This batch's scripts, workflow and docs.
        scripts/check-batch03-ownership.sh) return 0 ;;
        scripts/verify-raydium-idl.sh) return 0 ;;
        .github/workflows/venues.yml) return 0 ;;
        docs/RAYDIUM-VENUES.md) return 0 ;;
        docs/gap-updates/batch-03-rows.md) return 0 ;;
        docs/gap-updates/batch-03-wiring.md) return 0 ;;
        *) return 1 ;;
    esac
}

# Paths that must never change in ANY batch. These are checked explicitly so
# a regression is reported by name rather than as an opaque "forbidden path".
is_protected() {
    case "$1" in
        crates/module-sniper/*) return 0 ;;
        crates/server/*) return 0 ;;
        crates/core/*) return 0 ;;
        crates/module-telegram/*) return 0 ;;
        crates/module-copy/*) return 0 ;;
        crates/module-polymarket/*) return 0 ;;
        crates/core/migrations/*) return 0 ;;
        apps/*) return 0 ;;
        openapi/*) return 0 ;;
        Cargo.lock) return 0 ;;
        rust-toolchain.toml) return 0 ;;
        config.toml|config.toml.*) return 0 ;;
        .env.template) return 0 ;;
        .github/workflows/ci.yml) return 0 ;;
        docs/STATS.md) return 0 ;;
        release-manifest.json) return 0 ;;
        docs/KNOWN-LIMITATIONS.md) return 0 ;;
        OWNER-TODO.md) return 0 ;;
        GAP-MISSING-FILES.md|docs/GAP-MISSING-FILES.md) return 0 ;;
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
    if is_protected "$path"; then
        echo "check-batch03-ownership: PROTECTED path was modified: $path" >&2
        bad=1
        continue
    fi
    if ! is_allowed "$path"; then
        echo "check-batch03-ownership: FORBIDDEN changed path: $path" >&2
        bad=1
    fi
done

# (b) Every file listed in the target tree must exist and have >= 40 lines
# (`idl/*.json` and `fixtures/raydium/*.json` exempt from the line floor).
required=(
    crates/solana-kit/idl/raydium_cp_swap.json
    crates/solana-kit/idl/raydium_launchpad.json
    crates/solana-kit/idl/SOURCES.md
    crates/solana-kit/src/lib.rs
    crates/solana-kit/src/consts.rs
    crates/solana-kit/src/venues/mod.rs
    crates/solana-kit/src/venues/raydium_cpmm/mod.rs
    crates/solana-kit/src/venues/raydium_cpmm/pda.rs
    crates/solana-kit/src/venues/raydium_cpmm/accounts.rs
    crates/solana-kit/src/venues/raydium_cpmm/decode.rs
    crates/solana-kit/src/venues/raydium_cpmm/quote.rs
    crates/solana-kit/src/venues/raydium_cpmm/instructions.rs
    crates/solana-kit/src/venues/raydium_cpmm/adapter.rs
    crates/solana-kit/src/venues/raydium_launchlab/mod.rs
    crates/solana-kit/src/venues/raydium_launchlab/pda.rs
    crates/solana-kit/src/venues/raydium_launchlab/accounts.rs
    crates/solana-kit/src/venues/raydium_launchlab/decode.rs
    crates/solana-kit/src/venues/raydium_launchlab/curve.rs
    crates/solana-kit/src/venues/raydium_launchlab/instructions.rs
    crates/solana-kit/src/venues/raydium_launchlab/migration.rs
    crates/solana-kit/src/venues/raydium_launchlab/adapter.rs
    crates/solana-kit/fixtures/raydium/cpmm_pool_state.synthetic.json
    crates/solana-kit/fixtures/raydium/cpmm_quote_vectors.json
    crates/solana-kit/fixtures/raydium/launchlab_pool_state.synthetic.json
    crates/solana-kit/fixtures/raydium/launchlab_curve_vectors.json
    crates/solana-kit/tests/raydium_cpmm_discriminators.rs
    crates/solana-kit/tests/raydium_cpmm_pda.rs
    crates/solana-kit/tests/raydium_cpmm_accounts.rs
    crates/solana-kit/tests/raydium_cpmm_quote.rs
    crates/solana-kit/tests/raydium_cpmm_instructions.rs
    crates/solana-kit/tests/raydium_cpmm_decode.rs
    crates/solana-kit/tests/raydium_launchlab_accounts.rs
    crates/solana-kit/tests/raydium_launchlab_curve.rs
    crates/solana-kit/tests/raydium_launchlab_instructions.rs
    crates/solana-kit/tests/raydium_launchlab_decode.rs
    crates/solana-kit/tests/raydium_launchlab_migration.rs
    crates/solana-kit/tests/venue_registry_raydium.rs
    scripts/check-batch03-ownership.sh
    scripts/verify-raydium-idl.sh
    .github/workflows/venues.yml
    docs/RAYDIUM-VENUES.md
    docs/gap-updates/batch-03-rows.md
    docs/gap-updates/batch-03-wiring.md
)
for path in "${required[@]}"; do
    if [[ ! -f "$path" ]]; then
        echo "check-batch03-ownership: MISSING required deliverable: $path" >&2
        bad=1
        continue
    fi
    case "$path" in
        crates/solana-kit/idl/*.json|crates/solana-kit/fixtures/raydium/*.json)
            ;;
        *)
            lines="$(wc -l < "$path" | tr -d '[:space:]')"
            if (( lines < 40 )); then
                echo "check-batch03-ownership: TOO SHORT ($lines < 40 lines): $path" >&2
                bad=1
            fi
            ;;
    esac
done

# `Cargo.lock` must be byte-identical to the base commit's.
if ! git diff --quiet "$BASE" -- Cargo.lock; then
    echo "check-batch03-ownership: Cargo.lock was modified" >&2
    bad=1
fi

if (( bad )); then
    echo "check-batch03-ownership: FAIL" >&2
    exit 1
fi

echo "check-batch03-ownership: PASS — ${#changed[@]} changed file(s), all owned by Batch 03"
