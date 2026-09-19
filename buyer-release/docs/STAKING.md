# Staking program (Module 4)

Native Solana program (no Anchor), declared id
`3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy` (`staking_suite::ID`).
Single SPL-token staking pool with deposit fees, time-based reward minting,
a public parameter timelock, two-step admin transfer, a one-time latched
genesis mint, an IMMUTABLE on-chain max-supply cap enforced against every
mint, and one-shot SPL token metadata (mpl-token-metadata) creation.

## Economics

* **Deposit fee** `fee_bps` (cap: 1000 = 10%) — taken on every `Stake`,
  routed to the treasury token account. Principal net of fee goes to the
  vault (a token account owned by the config PDA).
* **Rewards** `reward_rate_bps` annual (cap: 10000 = 100%/yr), linear:
  `amount * rate_bps * elapsed_secs / (10_000 * 31_536_000)`, rounded down,
  settled on `Claim`/`Unstake` and **minted** (supply grows by exactly the
  payout). Top-ups settle accrued rewards first so nothing is lost.
* **Cooldown** `unstake_delay` seconds before principal can be withdrawn.
* **Pause** blocks deposits only — `Unstake`/`Claim` can never be paused, so
  the admin can halt new money during an incident but can never freeze user
  funds.
* **Max supply** `max_supply` (raw units, set at `Initialize`, must be > 0,
  **immutable afterwards** — deliberately not part of `UpdateParams`, so no
  admin action can raise it). Enforced against the LIVE mint supply (the SPL
  mint account is the authoritative total, not a self-tracked counter):
  * `GenesisMint` fails with `MaxSupplyExceeded` (6028) unless
    `supply + amount <= max_supply` (checked arithmetic; overflow fails
    closed);
  * reward minting (`Claim`/`Unstake`) is **clamped** to the remaining
    headroom `max_supply - supply`: withdrawals never fail, but once the cap
    is reached further rewards cannot be minted and the shortfall is
    forfeited (logged on-chain via `msg!`). Operators MUST size
    `max_supply` = genesis + the full intended reward budget.

## Governance

* `UpdateParams` (admin) queues a change; it becomes applicable only after
  `timelock_secs` (cap: 30 days) via `ApplyParams`, which is
  **permissionless** — anyone can push a published, expired update through,
  and caps are re-checked at apply time. `CancelParams` (admin) withdraws a
  queued update. Changing `timelock_secs` itself goes through the current
  timelock.
* `TransferAdmin` (admin proposes) + `AcceptAdmin` (proposed key signs) —
  two-step, so control can never be handed to an unowned key.
* `GenesisMint` (admin, **once per deployment**): mints the initial supply to
  a recipient token account and flips `Config::genesis_done`; every later
  attempt fails with `GenesisAlreadyDone` (error 6026). Bounded by
  `max_supply` (see Economics). This is the only sanctioned initial
  distribution — afterwards the mint authority (the config PDA) only ever
  mints accrued rewards, themselves clamped to the cap.
* `CreateTokenMetadata` (admin, **once per deployment**): CPI to
  mpl-token-metadata `CreateMetadataAccountsV3` creating the mint's metadata
  account (name ≤ 32 B, symbol ≤ 10 B, uri ≤ 200 B, none empty — validated
  before the CPI). The metadata is created **immutable**
  (`is_mutable = false`) with the config PDA as mint/update authority, so
  nobody — including a compromised admin — can rewrite it later. Replay
  fails with `MetadataAlreadyExists` (6030); a non-canonical metadata
  program account fails with `InvalidMetadataProgram` (6031); the metadata
  account must be the canonical mpl PDA
  `["metadata", metadata_program, mint]`.

## Accounts

| Account | Derivation | Contents |
|---|---|---|
| Config PDA | seeds `["staking-config"]` | `Config` (borsh): admin, mint, vault, treasury, params, pause, pending admin/update, genesis latch |
| Mint | keypair signer at `Initialize`; mint authority = config PDA; **no freeze authority** | SPL mint |
| Vault | ATA of config PDA on the mint | all staked principal |
| Treasury | ATA of the treasury wallet | collected fees |
| Stake PDA | seeds `["staking-stake", staker]` | `StakeAccount`: owner, amount, staked_at, reward_from, pending_rewards |
| Metadata PDA | mpl derivation `["metadata", metadata_program, mint]` (under the metadata program) | mpl `Metadata` (immutable; created by `CreateTokenMetadata`) |

## Instructions (borsh enum, discriminant = first byte)

`Initialize{... max_supply}` · `Stake{amount}` · `Unstake` · `Claim` ·
`UpdateParams{...}` · `ApplyParams` · `CancelParams` · `Pause` · `Unpause` ·
`TransferAdmin{new_admin}` · `AcceptAdmin` · `GenesisMint{amount}` ·
`CreateTokenMetadata{name,symbol,uri}`

Client builders for all of them live in `staking_suite::instruction`
(`stake_ix`, `unstake_ix`, `claim_ix`, `admin_ix`, `update_params_ix`,
`apply_params_ix`, `genesis_mint_ix`, `create_token_metadata_ix`). Errors
are `Custom(6000 + n)` — see `error.rs` for the full table (e.g. 6018
Paused, 6024 TimelockNotElapsed, 6026 GenesisAlreadyDone, 6027
InvalidAmount, 6028 MaxSupplyExceeded, 6029 InvalidMaxSupply, 6030
MetadataAlreadyExists, 6031 InvalidMetadataProgram, 6032
MetadataFieldTooLong).

## Deploy + launch sequence

```bash
cd programs/staking-suite
cargo build-sbf                       # agave 2.1.21 toolchain (see CI notes)
solana program deploy target/deploy/staking_suite.so \
  --program-id target/deploy/staking_suite-keypair.json   # or your fixed id
```

Then, as admin (payer of initialize becomes admin):

1. `Initialize { fee_bps, reward_rate_bps, min_stake, unstake_delay,
   decimals, timelock_secs, max_supply }` — creates mint (new keypair
   signs), vault, treasury, config PDA in one tx. Production should use
   `timelock_secs` ≥ 24h. `max_supply` is FINAL at this point (genesis +
   the entire reward budget); it can never be raised later.
2. Create the distribution wallet's ATA for the mint.
3. `GenesisMint { amount }` to that ATA — **once**, `amount ≤ max_supply`;
   verify `Config::genesis_done == true` and `Mint::supply == amount`
   afterwards.
4. `CreateTokenMetadata { name, symbol, uri }` (admin) — **once**; verify
   the metadata PDA exists under the mpl program and shows the intended
   name/symbol/uri in explorers. (The URI should point at a JSON file with
   `name`, `symbol`, `description`, `image`, matching the on-chain fields.)
5. Distribute tokens off-chain / via your sale process; verify on-chain
   balances match your records (the program cannot know about your sale).
6. Publish the mint address + program id for stakers.

## Testing status (honest)

* **Host unit tests:** instruction round-trips, borsh layouts, reward / fee
  math (incl. overflow saturation), signer/address/owner guards, timelock
  validation, genesis authorization/latch/amount/mint checks, max-supply
  cap math (exact-cap / one-over / live-supply authority / overflow fail-
  closed), reward clamping at zero and partial headroom, metadata field
  limits (byte lengths incl. multi-byte), metadata PDA/program/one-shot
  guards and the pinned `CreateMetadataAccountV3` (discriminant 33) byte
  layout.
  `cargo test` in `programs/staking-suite`.
* **Validator e2e (3 tests, gated behind `STAKING_E2E=1`) — ALL 3 EXECUTED
  AND PASSED (hardening pass 2026-09-18)** against a real
  `solana-test-validator` (Agave 2.1.21, platform-tools v1.43) running the
  audit-pass BPF binary (`staking_suite.so`, 187,504 bytes, SHA-256
  `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`):
  batch run `3 passed; 0 failed` in 160.72 s (log:
  `evidence/phase5-full-batch.log` in the delivery evidence package).
  Coverage: governance lifecycle (initialize, re-init guard, stake guards,
  pause authorization, timelock queue/apply/cancel with caps, two-step
  admin transfer); the funded money flow (genesis mint → stake with fee
  split → reward accrual → claim mints exactly the payout → unstake drains
  the vault; genesis replay → 6026; non-admin genesis → Unauthorized); and
  `validator_e2e_max_supply_cap_and_metadata` (zero-cap rejection, genesis
  one-over-cap → 6028, exactly-at-cap success, reward clamp with supply
  frozen at the cap, metadata creation against the REAL mpl-token-metadata
  program cloned from mainnet-beta + replay rejection).
  Executing the metadata e2e found and fixed two real defects: the harness
  now passes `--clone-upgradeable-program` (plain `--clone` copies the
  program account WITHOUT its programdata → "Program is not deployed"),
  and the on-chain CPI discriminant for `CreateMetadataAccountV3` was
  corrected 19 → **33** (verified against the mpl source deployed to
  mainnet-beta, tag `token-metadata@v1.14.0`, and then against the real
  cloned program). The earlier freeze-pass run (2/2, 84 s) remains valid
  as historical evidence for the two tests it covered.
  Run: `STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1`.
* **NOT done:** third-party audit, mainnet deployment, fuzzing. Reward math
  is linear/simple by design; caps are enforced at both queue and apply
  time. Do not claim this program is "audited" — it is well-tested, not
  audited.
