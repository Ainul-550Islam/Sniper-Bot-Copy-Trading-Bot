# Staking Suite — Threat Model

**Program:** `programs/staking-suite` (Module 4, native Solana, no Anchor)
**Scope:** the on-chain program only. Off-chain callers, wallet infrastructure,
and the metadata JSON behind the token URI are out of scope.
**Date:** 2026-10-08 · **Status:** static review (no cargo toolchain in this
workspace; companion host tests + `cargo build-sbf` run in CI)

---

## 1. Assets

| # | Asset | Where | Why it matters |
|---|---|---|---|
| A1 | Staked principal (SPL tokens) | Vault ATA owned by the config PDA | User funds; total = Σ `StakeAccount.amount` |
| A2 | Mint authority | Config PDA (`mint_bump`) | Whoever controls it can inflate the token |
| A3 | Parameter control | `Config.admin` | Fee, reward rate, min stake, cooldown, pause |
| A4 | Max-supply cap | `Config.max_supply` (immutable) | The last line of defense against unlimited minting |
| A5 | Deposit fees | Treasury ATA | Protocol revenue |
| A6 | Program state integrity | Config + stake PDAs (borsh) | Corrupted/bogus state breaks accounting |

## 2. Trust assumptions

1. **The SPL Token program is trusted.** All balance truth comes from SPL
   accounts; this program never trusts its own counters over the mint/vault.
2. **The mpl-token-metadata program is trusted** for the one-shot metadata
   CPI only. It can never move tokens or touch the vault: the CPI is confined
   to `CreateTokenMetadata`, which passes no token accounts and is signed by
   the mint keypair + admin.
3. **The admin key is semi-trusted.** The design goal is that even a fully
   compromised admin **cannot steal staked principal and cannot mint without
   bound** (see §4 mitigations M3/M4/M6/M7).
4. **Clock is trusted within Solana's normal guarantees.** Reward accrual and
   timelocks use `Clock.unix_timestamp`; validator clock skew is accepted risk
   (bounded by cluster consensus rules).

## 3. Threat actors

| Actor | Capability | Goal |
|---|---|---|
| T1 Arbitrary attacker | Can send any instruction, craft any account set | Steal vault tokens, mint tokens, corrupt state |
| T2 Compromised admin | Holds the admin key | Raise fees/rates instantly, mint without limit, rug |
| T3 Malicious staker | Controls one or many staker wallets | Double-count stake, claim others' rewards, break accounting |
| T4 Metadata-program adversary | Crafts the metadata CPI context | Redirect the CPI, exhaust compute, write oversized fields |
| T5 State-injection adversary | Submits transactions with fake config/stake accounts | Confuse PDA checks, substitute attacker-owned accounts |

## 4. Threats and mitigations

### T1 — Vault theft via instruction forgery
- **T1.1 Fake vault/treasury/mint passed in the account list.**
  *Mitigation:* every handler re-derives or compares against `Config`
  (`InvalidVault`, `InvalidMint`, `InvalidTreasury`); the vault is
  the ATA of the config PDA, and transfers are CPI'd with the config PDA
  signer seeds — an attacker-supplied token account receives nothing.
- **T1.2 Withdrawals to arbitrary destination.** *Mitigation:* unstake/claim
  require the destination token account to be owned by the signing staker and
  match the config mint (`require_staker_token`).
- **T1.3 Re-initialisation / config overwrite.** *Mitigation:* `initialized`
  latch (`AlreadyInitialized`) and PDA identity check on the config account
  (`InvalidConfigAccount`).

### T2 — Admin abuse (the core economic threat)
- **T2.1 Instant parameter change (fee → 100%).** *Mitigation:* all parameter
  changes go through `UpdateParams` → timelock → `ApplyParams` (with `CancelParams` to withdraw a queued change). Shortening
  the timelock itself is a queued change subject to the CURRENT delay
  (`timelock_secs` lives in `PendingParams`, never in the direct path).
- **T2.2 Fee confiscation.** *Mitigation:* hard cap `MAX_FEE_BPS = 1000`
  (10%) enforced in `validate_params` at both queue AND apply time
  (`FeeTooHigh`); the cap is a compile-time constant, not admin-writable.
- **T2.3 Unlimited reward minting.** *Mitigation:* `MAX_REWARD_RATE_BPS =
  10_000` caps APR; and regardless of rate, ALL minting is clamped to
  `max_supply - live mint supply` (A4). Once the cap is reached, rewards
  clamp to the remaining headroom instead of failing withdrawals.
- **T2.4 Genesis re-mint.** *Mitigation:* `genesis_done` latch; `GenesisMint`
  succeeds exactly once (`GenesisAlreadyDone`) and must fit under the cap
  (`MaxSupplyExceeded`, checked arithmetic — overflow fails closed).
- **T2.5 Admin hand-off to a wrong/lost key.** *Mitigation:* two-step
  `TransferAdmin` → `AcceptAdmin`; promotion requires the pending admin's
  signature (`NotPendingAdmin`).
- **T2.6 Freezing user funds via `Pause`.** *Mitigation (by design):*
  `Pause` sets the flag that blocks ONLY new deposits. `Unstake` and `Claim` never consult it,
  so the admin can halt inflows during an incident but can never trap funds.
- **Residual risk:** within the timelock window a malicious admin can queue
  changes up to the caps; users must monitor `Config.pending` (published
  on-chain for the whole window) and exit if they object. This is stated in
  the runbook; a 0-second timelock is legal but discouraged (cap
  `MAX_TIMELOCK_SECS` = 30 days).

### T3 — Malicious staker
- **T3.1 Claiming another staker's rewards.** *Mitigation:* the stake PDA is
  derived from `["staking-stake", staker]` and the staker must sign
  (`InvalidStakeAccount`); there is no instruction that reads someone
  else's PDA.
- **T3.2 Top-up without settling (reward clock manipulation).**
  *Mitigation:* `Stake` calls `settle()` before changing principal, folding
  live accrual into `pending_rewards` and resetting `reward_from` — top-ups
  neither lose nor double-count rewards.
- **T3.3 Dust spam / state bloat.** *Mitigation:* `min_stake` floor
  (`BelowMinimum`); each staker owns exactly one rent-paying PDA of
  fixed size.
- **T3.4 Early unstake.** *Mitigation:* `unstake_delay` cooldown checked
  against `Clock` (`CooldownActive`).

### T4 — Metadata CPI surface
- **T4.1 CPI redirection to a fake metadata program.** *Mitigation:* the
  handler requires the passed metadata program account to equal the compiled
  constant `TOKEN_METADATA_PROGRAM_ID` (`InvalidMetadataProgram`) and re-derives
  the metadata PDA itself.
- **T4.2 Oversized fields failing the CPI late (or worse, partially).**
  *Mitigation:* name ≤ 32, symbol ≤ 10, uri ≤ 200 bytes enforced before the
  CPI (`MetadataFieldTooLong`); the instruction is one-shot
  (`MetadataAlreadyExists`).
- **T4.3 Metadata accounts granted token privileges.** *Mitigation:* the CPI
  account list contains no vault/treasury/stake accounts; the mint signs only
  for metadata creation.

### T5 — State injection / deserialization
- **T5.1 Borsh bombs / trailing garbage.** *Mitigation:* deserialization uses
  fixed-schema borsh; failures return `InvalidInstructionData` — no
  panics (see INVARIANTS.md §3).
- **T5.2 Account re-use (same account passed twice in different slots).**
  *Mitigation:* every slot is independently checked for key, owner, and
  (where relevant) PDA seeds; aliasing a checked account into another slot
  fails the second slot's identity check.
- **T5.3 Arithmetic overflow as an attack.** *Mitigation:* all arithmetic is
  checked/saturating (`Overflow`, `Arithmetic`); `compute_reward`
  saturates to `u64::MAX` rather than panicking; the proptest suite covers
  the boundaries.

## 5. Out-of-scope / accepted risks

| Risk | Rationale |
|---|---|
| `Clock.unix_timestamp` skew | Bounded by cluster rules; rewards/timelocks shift by at most the skew |
| Admin queues changes right up to caps | Timelock + on-chain `pending` publication gives users an exit window; caps bound the worst case |
| Token price/market effects | Not an on-chain property |
| Metadata JSON content (URI target) | Off-chain; the chain stores only the URI string |
| Denial of service via compute | Each instruction is O(1) accounts and fixed-size state; no loops over unbounded collections |

## 6. Testing map (where each threat is exercised)

| Threat | Test |
|---|---|
| T1.x, T3.x, T5.2 | `tests/validator_e2e.rs` (STAKING_E2E=1, real validator), host tests in `src/processor.rs` |
| T2.x | host tests: caps, timelock, two-step admin, pause semantics; `tests/property_rewards.rs` (proptest) |
| T4.x | `src/processor.rs` metadata tests + `state.rs` PDA-derivation tests |
| T5.1/T5.3 | `fuzz/fuzz_targets/staking_deserialize.rs`, `staking_math.rs` |

No item in this model is marked "verified by live audit"; this is a static
review artifact. Live evidence belongs under `evidence/live/` and is
`NOT_RUN` until a real execution produces it.
