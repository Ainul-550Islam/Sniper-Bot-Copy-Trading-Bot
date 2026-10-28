# Batch 03 — wiring notes

This file records how the two Raydium venue modules are meant to be wired
into the rest of the system, and — just as importantly — what this batch
deliberately did **not** wire. It is a handoff note for Batch 07 (the sniper
pipeline) and for the integration step.

---

## 1. What is wired now

Everything in this section is inside `crates/solana-kit`, which is this
batch's only owned crate.

```
crates/solana-kit/src/venues/
├── mod.rs                      ← VenueRegistry::default_registry()
├── raydium_cpmm/               ← NEW (replaces the raydium_cpmm.rs stub)
│   ├── mod.rs
│   ├── pda.rs                  ← the single owner of CPMM PDA derivation
│   ├── accounts.rs             ← PoolState (637 B) / AmmConfig (236 B)
│   ├── decode.rs               ← SwapEvent + initialize (pool-creation fallback)
│   ├── quote.rs                ← the constant-product quote
│   ├── instructions.rs         ← swap_base_input / swap_base_output
│   └── adapter.rs              ← impl VenueAdapter
├── raydium_launchlab/          ← NEW
│   ├── mod.rs
│   ├── pda.rs                  ← the single owner of LaunchLab PDA derivation
│   ├── accounts.rs             ← PoolState (429 B) / GlobalConfig (371 B) / …
│   ├── decode.rs               ← TradeEvent / PoolCreateEvent / initialize_v2
│   ├── curve.rs                ← the three curve types + the fee maths
│   ├── instructions.rs         ← buy/sell × exact-in/exact-out
│   ├── migration.rs            ← the graduation hand-off
│   └── adapter.rs              ← impl VenueAdapter
└── meteora.rs                  ← UNCHANGED, still unverified
```

The registry order is:

```rust
adapters: vec![
    Box::new(raydium_cpmm::RaydiumCpmm::new()),
    Box::new(raydium_launchlab::RaydiumLaunchLab::new()),
    Box::new(meteora::MeteoraDlmm::new()),
    Box::new(meteora::MeteoraDammV1::new()),
]
```

`VenueRegistry::buildable()` returns only the adapters whose
`layout_verified()` is `true` **and** which are `enabled()`. After this batch
that is exactly CPMM and LaunchLab; Meteora stays out because its layouts are
still unverified.

The two new adapters replace the previous CPMM stub, which returned
`layout_verified() == false` and refused every build. The CPMM adapter now
returns `true`, and its `build_swap` builds a real instruction.

---

## 2. What is deliberately NOT wired

| Not wired | Why | Who |
|---|---|---|
| The sniper pipeline → CPMM/LaunchLab | Out of scope for this batch. | Batch 07 |
| Any HTTP route | This batch adds no route. | — |
| Any OpenAPI entry | Same. | — |
| Any frontend file | Same. | — |
| `SwapVenue::RaydiumLaunchLab` | `SwapVenue` is matched **exhaustively** by `crates/module-copy/src/feeds.rs`, which this batch may not touch. Adding a variant breaks `module-copy`'s build. | A batch that owns `module-copy` |
| The Meteora layouts | Still unverified; the adapters keep refusing. | a future venue batch |
| A live on-chain capture | No live capture was performed, so no live evidence is claimed. | integration / ops |

### The `SwapVenue` consequence, in detail

`crates/solana-kit/src/decode.rs` defines:

```rust
pub enum SwapVenue { /* …, RaydiumCpmm, …, Other }
```

`crates/module-copy/src/feeds.rs` matches it exhaustively. This batch may not
add a variant, so:

* **CPMM** → `SwapVenue::RaydiumCpmm` (the variant already exists).
* **LaunchLab** → `SwapVenue::Other`.

A consumer that needs to tell a LaunchLab trade apart must use the
LaunchLab-specific type:

```rust
use solana_kit::venues::raydium_launchlab::decode::LaunchLabTradeEvent;

let ev = LaunchLabTradeEvent::decode_base64(payload)?;
if ev.is_graduated() { /* the bonding curve graduated */ }
```

`LaunchLabTradeEvent` carries the pool, the amounts, the fee split, the pool
status and the trade direction — strictly more than the neutral `DecodedSwap`
carries. The conversion site documents why it reports `Other`:

```rust
// `SwapVenue` is a pre-existing public enum that `module-copy` matches
// exhaustively, and `module-copy` is owned by another batch. Adding a
// `RaydiumLaunchLab` variant here would break that crate's build, so
// the neutral conversion reports the generic `Other` venue instead.
```

When the batch that owns `module-copy` adds the variant, the fix is a
one-line change here plus a `module-copy` arm — not a redesign.

---

## 3. How Batch 07 should use these modules

### 3.1 Detection

For a CPMM pool, the layout-free fact is enough to answer "does a CPMM pool
exist for this mint?": the pool account's **owner** is the CPMM program.

```rust
let adapter = raydium_cpmm::RaydiumCpmm::new();
if adapter.is_pool_owner(&account.owner) { /* it is a CPMM pool */ }
```

For LaunchLab, the pool's PDA is derived from the seeds in
`raydium_launchlab::pda::pool_pda(...)`, so detection is a derivation, not a
scan.

### 3.2 Quoting

Both modules expose a quote that is an **exact integer port** of the
on-chain maths, with `u128` intermediates and no floating point:

```rust
let quote = raydium_cpmm::quote::swap_base_input(&pool, &config, amount_in)?;
let quote = raydium_launchlab::curve::ConstantProductCurve::buy_exact_in(&state, amount)?;
```

Both refuse the inputs the on-chain program refuses (zero denominators,
impossible reserves, a fee rate at or above the denominator).

### 3.3 Building

Both expose a builder that produces a `solana_sdk::instruction::Instruction`
with the official account order and the official data layout.

```rust
let ix = raydium_cpmm::instructions::build_swap_base_input(
    &pool, &amm_config, &authority, &observation,
    &input_vault, &output_vault, &accounts, amount_in, min_out,
)?;
```

The CPMM builders **derive the authority from the pool's stored bump** and
refuse a bump that does not produce a valid program address — they never
guess one.

### 3.4 The LaunchLab graduation hand-off

The migration decision is a pure function:

```rust
use solana_kit::venues::raydium_launchlab::migration::{decide_route, LaunchLabRoute};

let decision = decide_route(&pool);
match decision.route {
    LaunchLabRoute::Curve => { /* still on the bonding curve */ }
    LaunchLabRoute::Cpmm  => { /* hand off to the CPMM adapter */ }
    LaunchLabRoute::AmmV4 => { /* hand off to the AMM v4 adapter */ }
}
```

`decision.target` names the **adapter** to use, not a pool address. The
migrated pool's address is not recorded in the LaunchLab `PoolState`, so it
cannot be derived from the state alone — Batch 07 must resolve it from the
`migrate_to_*` transaction (the instruction carries the new pool's address in
its account list).

`decision.threshold_met` reports whether the SDK's liquidity threshold
(`floor(sqrt(migrate_amount_a * total_fund_raising_b))`, compared *strictly
greater* than `10^decimals` for AMM v4 and `100` for CPMM) has been reached.

---

## 4. The gates that must stay green

```bash
bash scripts/verify-raydium-idl.sh        # the vendored IDLs are still the pinned upstream ones
bash scripts/check-batch03-ownership.sh    # this batch touched nothing it does not own
cargo test --locked -p solana-kit          # the venue tests
cargo check  --workspace --all-targets --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt    --all --check
cargo test   --locked -p module-sniper -p module-copy
bash scripts/check-openapi-coverage.sh
bash scripts/check-routes-vs-ui.sh
```

`scripts/check-openapi-coverage.sh` and `scripts/check-routes-vs-ui.sh` are
unaffected by this batch: it adds no HTTP route and no frontend file.

The CI workflow that runs the venue gates is
`.github/workflows/venues.yml`. It is offline and needs no secrets.
