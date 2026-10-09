# Staking Suite — Invariants

Machine-checkable properties of `programs/staking-suite`. Each invariant lists
**what must hold**, **where it is enforced**, and **what tests/fuzzers guard
it**. If any invariant is found to be false, that is a P0 bug — stop and fix
before any deployment.

Legend: ✅ enforced in code · 🧪 covered by a test · 🌀 covered by a fuzz target.

---

## A. Authorization & identity

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| A1 | Only the config PDA `["staking-config"]` is accepted as config | `load_config` + `config_pda` re-derive | ✅🧪 |
| A2 | Only `["staking-stake", staker]` is accepted as a stake account, and `staker` signs | `stake_pda` re-derive + `require_signer` | ✅🧪 |
| A3 | Admin-gated instructions (`UpdateParams`, `Pause`, `Unpause`, `TransferAdmin`, `GenesisMint`, `CreateTokenMetadata`) are signed by `Config.admin` | `require_signer` against `config.admin` | ✅🧪 |
| A4 | `AcceptAdmin` is signed by `Config.pending_admin`, not the current admin | `process_accept_admin` | ✅🧪 |
| A5 | Vault/treasury/mint in the account list must equal the config's recorded addresses | `require_address` → `InvalidVault`/`InvalidTreasury`/`InvalidMint` | ✅🧪 |
| A6 | Staker token accounts are owned by the signing staker and match the config mint | `require_staker_token` | ✅🧪 |

## B. Funds safety

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| B1 | Total staked principal never exceeds the vault's SPL balance | Only `Stake` adds to vault (CPI transfer in); `Unstake` returns ≤ `sa.amount` | ✅🧪 |
| B2 | A staker can never withdraw more than `sa.amount + accrued_rewards` | `process_unstake` computes both from the staker's own PDA | ✅🧪 |
| B3 | Rewards are minted (not taken from the vault); vault principal is untouched by `Claim`/reward minting | `Claim`/`Unstake` mint via CPI with `mint_bump` seeds | ✅🧪 |
| B4 | Deposit fee ≤ 10% (hard cap, compile-time constant) | `validate_params` → `FeeTooHigh` at queue AND apply | ✅🧪 |
| B5 | Annual reward rate ≤ 100% APR (hard cap, compile-time constant) | `validate_params` → `RewardRateTooHigh` at queue AND apply | ✅🧪 |
| B6 | Fees are routed only to the treasury account, never to arbitrary accounts | treasury address pinned in config (A5) | ✅🧪 |
| B7 | `paused` blocks only `Stake`; `Unstake`/`Claim` always remain possible | `process_stake` checks `config.paused`; withdrawal paths do not | ✅🧪 |

## C. Supply & minting

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| C1 | `max_supply` is set once at `Initialize` (> 0) and immutable — not reachable via `UpdateParams` | absent from `PendingParams`; `InvalidMaxSupply` | ✅🧪 |
| C2 | Total mint supply never exceeds `max_supply` | Genesis: `fits_under_cap` → `MaxSupplyExceeded`; rewards: `supply_headroom` clamp | ✅🧪🌀 |
| C3 | `GenesisMint` executes at most once per deployment | `genesis_done` latch → `GenesisAlreadyDone` | ✅🧪 |
| C4 | Supply-cap checks use the LIVE SPL mint supply, not a self-tracked counter | `process_genesis_mint`/reward paths read the mint account | ✅ |
| C5 | Cap arithmetic never wraps: `supply + amount` overflow fails closed | `checked_add` in `fits_under_cap`; saturating headroom | ✅🧪🌀 |
| C6 | Hitting the cap never blocks withdrawals — rewards clamp to headroom, shortfall logged via `msg!` | `process_unstake`/`claim` clamp `rewards.min(headroom)` | ✅🧪 |

## D. Time & parameters

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| D1 | Parameter changes require a two-phase queue→apply flow; no direct writes | `UpdateParams` stores `pending`; only `ApplyParams` copies to live config | ✅🧪 |
| D2 | `ApplyParams` fails before `queued_at + timelock_secs` | `TimelockNotElapsed` | ✅🧪 |
| D3 | Shortening the timelock is itself subject to the CURRENT delay | new `timelock_secs` travels inside `PendingParams` | ✅🧪 |
| D4 | Timelock delay ∈ [0, 30 days] | `validate_timelock` → `TimelockOutOfRange` | ✅🧪 |
| D5 | At most one queued update; queueing over an active queue fails | `UpdateAlreadyQueued` / cleared by `ApplyParams`/`CancelParams` | ✅🧪 |
| D6 | Unstake respects the cooldown from the last principal change | `now - sa.staked_at >= unstake_delay` → `CooldownActive` | ✅🧪 |
| D7 | Reward accrual is linear, monotone in time, and rounds down | `compute_reward` integer division | ✅🧪🌀 |
| D8 | Top-ups settle pending rewards before changing principal (no loss, no double count) | `StakeAccount::settle` before `amount` mutation | ✅🧪🌀 |

## E. Arithmetic & robustness

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| E1 | No instruction panics on any input; all failures are typed `ProgramError`s | checked/saturating arithmetic everywhere; fuzz targets | ✅🧪🌀 |
| E2 | Reward/fee numerators are computed in u128 and never overflow u64 silently | `checked_mul` in u128; saturate to `u64::MAX` | ✅🧪🌀 |
| E3 | Fee rounds down (never charges more than `fee_bps` warrants) | integer division in `compute_fee` | ✅🧪🌀 |
| E4 | Malformed instruction data fails to deserialize cleanly | `InvalidInstructionData` | ✅🧪🌀 |
| E5 | State round-trips through borsh without loss | `Config`/`StakeAccount` derive both directions | ✅🧪 |

## F. One-shot metadata surface

| # | Invariant | Enforcement | Guard |
|---|---|---|---|
| F1 | Metadata CPI targets only the canonical mpl-token-metadata program id | `InvalidMetadataProgram` | ✅🧪 |
| F2 | Metadata PDA matches `["metadata", metadata_program, mint]` exactly | `metadata_pda` re-derive + `require_address` | ✅🧪 |
| F3 | `CreateTokenMetadata` runs at most once | `MetadataAlreadyExists` | ✅🧪 |
| F4 | name ≤ 32, symbol ≤ 10, uri ≤ 200 bytes, non-empty | `MetadataFieldTooLong` / empty checks before CPI | ✅🧪 |

---

## Property-test coverage (`tests/property_rewards.rs`, proptest)

- `compute_reward` is monotone in `amount`, `rate_bps`, and `elapsed_secs`.
- `compute_reward(amount, rate, SECS_PER_YEAR) ≈ amount * rate / BPS` (within 1 unit).
- `compute_fee(amount, fee_bps) = floor(amount*fee_bps/BPS)` exactly, and
  `≤ amount` whenever `fee_bps ≤ 10_000` (the 10% cap is enforced by
  `validate_params`, not by the pure fee function).
- `fits_under_cap` ↔ `supply + amount ≤ cap` with overflow failing closed.
- `supply_headroom` saturates and never wraps.
- `settle()` conservation: `pending_before + accrued == pending_after`.

## Fuzz coverage (`fuzz/fuzz_targets/`)

- `staking_math.rs` — arbitrary `(amount, rate, elapsed, fee_bps, supply, cap)`
  tuples through `compute_reward`/`compute_fee`/`fits_under_cap`/
  `supply_headroom`; asserts no panic + E2/E3/C5 bounds.
- `staking_deserialize.rs` — arbitrary byte streams into
  `StakingInstruction::try_from_slice`, `Config`, and `StakeAccount`; asserts
  clean accept/reject (E4) and borsh round-trip on accepted values (E5).

Run: `cd programs/staking-suite && cargo fuzz run staking_math` (and
`staking_deserialize`). These targets are additive hardening; the invariants
above are enforced by the program regardless of whether a fuzzer is run.
