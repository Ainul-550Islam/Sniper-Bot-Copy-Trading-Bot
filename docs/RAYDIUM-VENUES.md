# Raydium venues — CPMM and LaunchLab

**Batch 03.** This document describes the two Raydium venue modules that
`solana-kit` gained in this batch, where every on-chain fact came from, and
what is deliberately **not** here yet.

---

## 1. What landed

| Module | Path | Program id | Layout |
|---|---|---|---|
| Raydium CPMM (`cp-swap`) | `crates/solana-kit/src/venues/raydium_cpmm/` | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | **verified** |
| Raydium LaunchLab (`launchpad`) | `crates/solana-kit/src/venues/raydium_launchlab/` | `LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj` | **verified** |

Both are registered in `VenueRegistry::default_registry()`
(`crates/solana-kit/src/venues/mod.rs`), in front of the Meteora adapters,
which are still unverified and therefore still refuse to build.

Each module has the same seven/eight-file shape:

```
mod.rs          module docs + re-exports
pda.rs          the single owner of PDA derivation for this venue
accounts.rs     account decoding (fail-closed) + synthetic encoders
decode.rs       event / instruction decoding into the neutral types
curve.rs        (LaunchLab only) the three curve types and the fee maths
quote.rs        (CPMM only) the constant-product quote
instructions.rs the instruction builders
adapter.rs      the `VenueAdapter` implementation
migration.rs    (LaunchLab only) the bonding-curve → AMM hand-off
```

Nothing outside `crates/solana-kit` was touched. In particular the sniper
pipeline is **not** wired to these venues yet — that is Batch 07.

---

## 2. Provenance: where every on-chain fact came from

No program id, account list, instruction argument or discriminator in this
batch was guessed. Each came from one of these three official sources:

| What | Repository | Commit |
|---|---|---|
| The IDLs (vendored) | `raydium-io/raydium-idl` | `e7e0c96fe77bcf6a020b84a44c47a722aac8e359` |
| The CPMM program (on-chain truth) | `raydium-io/raydium-cp-swap` | `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5` |
| The LaunchLab SDK (curve maths + seeds) | `raydium-io/raydium-sdk-V2` | `f8549145afe75d4c486e30c43331c2c4da32b1eb` |

The two IDLs are vendored verbatim into `crates/solana-kit/idl/`:

| File | sha256 |
|---|---|
| `raydium_cp_swap.json` | `1202f6dc8e1c3216598f2ad5c620b9aa8c64ac584563fafed68125c27fb6df81` |
| `raydium_launchpad.json` | `963189409945069409f32e345d4adf2892e0b3ca1be504372577fe34a23c6cdb` |

`crates/solana-kit/idl/SOURCES.md` records the upstream paths, the raw URLs,
the commits and the hashes. `scripts/verify-raydium-idl.sh` recomputes the
hashes and re-checks the program ids and the Anchor sighash convention; it is
offline and deterministic.

### The discriminator convention

Every instruction, account and event discriminator in `consts.rs` is asserted
in the test suite to be **both**:

1. `sha256("global:<snake_name>")[..8]` for instructions,
   `sha256("account:<Name>")[..8]` for accounts,
   `sha256("event:<Name>")[..8]` for events, and
2. byte-for-byte equal to the `discriminator` field in the vendored IDL.

So an upstream IDL edit that changes a discriminator fails the build rather
than silently producing a transaction the program will reject.

---

## 3. Raydium CPMM (`cp-swap`)

### 3.1 The account layouts

| Account | Length | Source |
|---|---|---|
| `PoolState` | **637** bytes | `programs/cp-swap/src/states/pool.rs` → `PoolState::LEN` |
| `AmmConfig` | **236** bytes | `programs/cp-swap/src/states/config.rs` → `AmmConfig::LEN` |

`PoolState` is decoded in this order (10 pubkeys, then the scalars, then the
creator-fee block):

```
pool_id, token_0_mint, token_1_mint, lp_mint, pool_vault_0, pool_vault_1,
owner, lp_mint_authority, token_0_program, token_1_program
u8  auth_bump
u8  status
u8  lp_mint_decimals
u8  token_0_mint_decimals
u8  token_1_mint_decimals
u64 token_0_vault_amount
u64 token_1_vault_amount
u64 lp_supply
u64 fee_growth_global_0
u64 fee_growth_global_1
u64 total_fee_0
u64 total_fee_1
u8  creator_fee_on
bool creator_fee_on_input
[6] padding
u64 creator_fee_token_0
u64 creator_fee_token_1
[u64; 28] padding
```

`AmmConfig` is:

```
u8   bump
bool  disable_flag
u16  index
u64  trade_fee_rate
u64  protocol_fee_rate
u64  fund_fee_rate
u64  creator_fee_rate
pubkey fee_owner
pubkey trade_fee_owner
u64  creator_fee_token_0
u64  creator_fee_token_1
[u64; 14] padding
```

> **Note on `creator_fee_share_rate`.** The on-chain `AmmConfig` carries a
> `creator_fee_share_rate` field that the *published IDL type* does not
> expose. The decoder follows the on-chain struct (which is what the program
> actually reads) and fails closed on any length that does not match exactly.

### 3.2 The quote maths

Ported line-for-line from `programs/cp-swap/src/curve/{constant_product,
calculator,fees}.rs` @ `f4b9b26`.

Without fees:

```
exact-in   out = floor(amount_in * reserve_out / (reserve_in + amount_in))
exact-out  in  = ceil(reserve_in * amount_out / (reserve_out - amount_out))
```

With fees, the fee denominator is **1 000 000** (hundredths of a bip):

* the trade fee and the creator fee are **ceiled**;
* the protocol fee and the fund fee are **floored**;
* the gross-up of the input amount is **ceiled**;
* the creator fee is charged on the **input** when `creator_fee_on_input` is
  set, otherwise on the **output**;
* when the creator fee is charged on the input it is taken as
  `floor(total_fee * creator_rate / (trade_rate + creator_rate))`.

Token-2022 transfer fees are deducted from the input (exact-in) or from the
output (exact-out); the exact-out path inverts the transfer-fee calculation
so the requested output is exactly what the user receives.

All intermediates are `u128` and every operation is checked. There is no
floating point anywhere in the quote path.

### 3.3 The instruction builders

`build_swap_base_input` and `build_swap_base_output` produce the two
Anchor instructions `swap_base_input` and `swap_base_output`, with the account
order taken from the IDL:

```
0 payer               signer, READ-ONLY
1 authority           readonly
2 amm_config          readonly
3 pool_state          writable
4 input_token_account writable
5 output_token_account writable
6 input_vault         writable
7 output_vault        writable
8 input_token_program readonly
9 output_token_program readonly
10 input_token_mint   readonly
11 output_token_mint  readonly
12 observation_state  writable
```

> **`payer` is signer-only.** The first version of this batch marked the
> payer writable. The official IDL and the Anchor source both declare it
> `signer` and **not** `writable` — the program never debits it, it only
> signs. The builders and the tests now use
> `AccountMeta::new_readonly(payer, true)`.

The argument layout is `amount: u64`, `other_amount_threshold: u64` — the
threshold is the minimum output for an exact-in swap and the maximum input
for an exact-out swap.

### 3.4 The PDAs

| Account | Seeds |
|---|---|
| authority | `vault_and_lp_mint_auth_seed` |
| pool | `pool`, `amm_config`, `token_0_mint`, `token_1_mint` |
| LP mint | `pool_lp_mint`, `pool_state` |
| vault | `pool_vault`, `pool_state`, `mint` |
| AmmConfig | `amm_config`, `index` |
| observation | `observation`, `pool_state` |
| permission | `permission`, `owner` |

> **The AmmConfig index is big-endian.** The on-chain program uses
> `index.to_be_bytes()` (`programs/cp-swap/src/instructions/admin/create_config.rs`).
> The Raydium SDK's `getCpmmPdaAmmConfigId` uses `u16ToBytes`, which is
> little-endian. The two agree for index `0` — the only config Raydium
> currently ships — and disagree for every other index. **This module follows
> the program**, because the program is what validates the seeds. The
> discrepancy is documented in `pda.rs` and asserted in the tests.

---

## 4. Raydium LaunchLab (`launchpad`)

### 4.1 The account layouts

| Account | Length |
|---|---|
| `PoolState` | **429** bytes |
| `GlobalConfig` | **371** bytes |
| `PlatformConfig` | **944** bytes (minimum) |
| `VestingRecord` | **160** bytes |

### 4.2 The three curve types

| Type | Code | Curve |
|---|---|---|
| `ConstantCurve` | `0` | constant product |
| `FixedCurve` | `1` | fixed price |
| `LinearCurve` | `2` | linear price (Q64) |

The curve maths is an exact integer port of
`raydium-sdk-V2/src/raydium/launchpad/curve/**`
(`constantProductCurve.ts`, `linearPriceCurve.ts`, `fixedPriceCurve.ts`,
`bignumber.ts`, `transfer.ts`).

**Constant product** — buy input `virtual_quote + real_quote`, output
`virtual_base - real_base`; sell input `virtual_base - real_base`, output
`virtual_quote + real_quote`. Exact-in floors, exact-out ceils.

**Fixed price** — exact-in `floor(init_output * amount_in / init_input)`;
exact-out `ceil(init_input * amount_out / init_output)`.

**Linear price** — uses Q64 and the official `sqrt` formulation:

```
buy  exact-in   floor(sqrt(2 * (real_quote + amount) * Q64 / virtual_base)) - real_base
buy  exact-out  ceil(virtual_base * (real_base + amount)^2 / (2 * Q64)) - real_quote
sell exact-in   ceil(...)  — the mirror of the buy
sell exact-out  floor(...) — the mirror of the buy
```

### 4.3 The fees

The total fee rate is the **sum** of the protocol, platform, share and
creator rates. The sum may reach the denominator (`1 000 000`) but must not
exceed it; the gross-up is undefined at or above the denominator and is
refused.

The fee is ceiled, the gross-up is ceiled, the individual components are
floored, and the protocol receives the remainder so the split always adds up
to the total.

### 4.4 The instruction builders

Four instructions, all with the same 15-account IDL prefix and the same
three SDK trailing accounts:

```
0 payer               signer, writable
1 authority           readonly
2 global_config       readonly
3 platform_config     readonly
4 pool_state          writable
5 user_base_token     writable
6 user_quote_token    writable
7 base_vault          writable
8 quote_vault         writable
9 base_token_mint     readonly
10 quote_token_mint   readonly
11 base_token_program readonly
12 quote_token_program readonly
13 event_authority     readonly
14 program             readonly
[share_fee_receiver]  writable, optional
system_program        readonly
platform_claim_fee_vault  writable
creator_claim_fee_vault   writable
```

The three arguments are `u64`, `u64`, `u64`. The IDL names them
`amount_in` / `minimum_amount_out` for the exact-in trades and
`amount_out` / `maximum_amount_in` for the exact-out trades; the third is
always `share_fee_rate`.

### 4.5 The migration hand-off

When a LaunchLab pool graduates, its tokens move to an AMM. `migration.rs`
decides **where** they go, as a pure function of the decoded pool state:

| Pool status | Route | Target |
|---|---|---|
| `Fund` | `LaunchLabRoute::Curve` | `MigrationTarget::LaunchLabCurve` |
| `Migrate` | `LaunchLabRoute::Curve` | `MigrationTarget::LaunchLabCurve` |
| `Trade` + `migrate_type == 1` | `LaunchLabRoute::Cpmm` | `MigrationTarget::RaydiumCpmm` |
| `Trade` + `migrate_type == 0` | `LaunchLabRoute::AmmV4` | `MigrationTarget::RaydiumAmmV4` |

The migration liquidity is the SDK's
`floor(sqrt(migrate_amount_a * total_fund_raising_b))`, and the threshold is
strictly greater than the SDK's minimum: `10^decimals` for AMM v4 and `100`
for CPMM.

The hand-off deliberately names the **adapter**, not a pool address: the
migrated pool's address is not recorded in the LaunchLab `PoolState`, so it
cannot be derived from the state alone. Batch 07 resolves it from the
`migrate_to_*` transaction.

---

## 5. The neutral `SwapVenue` constraint

`SwapVenue` is a **pre-existing public enum** in `crates/solana-kit/src/decode.rs`
that `crates/module-copy/src/feeds.rs` matches **exhaustively**. `module-copy`
is owned by another batch, so this batch may not add a variant to it — doing
so would break that crate's build.

Therefore:

* CPMM reports `SwapVenue::RaydiumCpmm` (that variant already exists);
* LaunchLab reports the generic `SwapVenue::Other`.

The LaunchLab-specific detail — the pool, the amounts, the fee split, the
pool status and the graduation signal — is preserved in
`LaunchLabTradeEvent`, which is the type a consumer should use. This is
recorded in the source at the conversion site.

---

## 6. Fixtures

Four committed JSON fixtures live under `crates/solana-kit/fixtures/raydium/`:

| File | Contents |
|---|---|
| `cpmm_pool_state.synthetic.json` | a synthetic CPMM `PoolState` + `AmmConfig` |
| `cpmm_quote_vectors.json` | CPMM quote vectors (with and without the creator fee) |
| `launchlab_pool_state.synthetic.json` | a synthetic LaunchLab `PoolState` + `GlobalConfig` |
| `launchlab_curve_vectors.json` | vectors for all three curve types |

Every fixture is explicitly labelled **synthetic** — each carries a
`"synthetic": true` flag and a `_comment` that says in plain words that it is
not mainnet data. Every fixture vector is re-computed and asserted byte-for-byte
by the `raydium_cpmm_{accounts,quote}` and `raydium_launchlab_{accounts,curve}`
integration tests.

No test in this batch touches the network, and no test contains a real key.

---

## 7. How to verify this batch

```bash
# The vendored IDLs still match their pinned upstream provenance.
bash scripts/verify-raydium-idl.sh

# This batch touched nothing it does not own.
bash scripts/check-batch03-ownership.sh

# The venue tests.
cargo test --locked -p solana-kit

# The workspace gates.
cargo check  --workspace --all-targets --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt    --all --check
cargo test   --locked -p module-sniper -p module-copy
```

The route/UI coverage gates that already exist in the repository
(`scripts/check-openapi-coverage.sh`, `scripts/check-routes-vs-ui.sh`) are
unaffected: this batch adds no HTTP route and no frontend.
