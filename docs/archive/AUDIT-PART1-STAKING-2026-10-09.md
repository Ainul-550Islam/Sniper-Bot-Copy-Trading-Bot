# Part 1 Audit — `programs/staking-suite` — 2026-10-09

Scope: `programs/staking-suite` (native solana-program 2.x, no Anchor).
Files reviewed in full: `src/processor.rs` (production code, lines 1–1249),
`src/state.rs`, `src/lib.rs`, `Cargo.toml`, `.cargo/config.toml`, `docs/STAKING.md`,
`docs/STAKING-PROGRAM-ID-VALIDATION.md`, `audit/INVARIANTS.md`.

## Validation results (real runs)

Toolchain: rustc/cargo 1.98.1 (pinned in `rust-toolchain.toml`).

| Check | Result |
|---|---|
| `cargo test --locked --all-targets` | 71 unit + 8 property + 3 validator-e2e: **all pass**, 0 failed |
| `cargo clippy --all-targets -- -D warnings` | **0 warnings / 0 errors** (after fix F-1) |
| Validator e2e tests | **3 SKIPPED** — each returns early unless `STAKING_E2E=1`, which also needs `cargo build-sbf` and `solana-test-validator`. Not available in this sandbox. Under the zero-skip policy this is an open item, not a pass. |
| `cargo build-sbf` (on-chain BPF object) | **Not run** — Solana platform-tools not installed. |

Environment note: the repo-root `.cargo/config.toml` sets `linker=clang` with
`-fuse-ld=lld`. This sandbox has no `clang`/`lld` and no root access, so I ran the
builds with `RUSTFLAGS=" "` (env var overrides that config). This is an
environment workaround, not a code change. The Dockerfile does not copy that
config, so it is unaffected.

## Fixes applied

**F-1 — clippy `absurd_extreme_comparisons` (tests/property_rewards.rs:72).**
`prop_assert!(r <= u64::MAX)` was a tautology. Replaced with an independent
128-bit model that checks the exact saturating result. This makes the property
test actually verify the saturation behaviour.

**F-2 — `Cargo.lock` drift.** The committed lock did not satisfy `cargo test --locked`
(it lacked `proptest` and its transitive deps). Regenerated minimally with
`cargo test` (the lock was not regenerated from scratch). `--locked` now passes.

## Findings not yet fixed (need decision or on-chain testing)

| ID | Sev | Location | Finding | Proposed fix |
|---|---|---|---|---|
| S-1 | **High** (if deploy and Initialize are separate txs) | `processor.rs` `process_initialize` (~L292) | `Initialize` does not check that the payer is the program's upgrade authority. The config PDA is deterministic, so the **first caller** after deployment becomes admin. `docs/STAKING.md` says the sequence is deploy, then Initialize as separate steps. | Require the payer to equal the ProgramData `upgrade_authority_address` (parse the `bpf_loader_upgradeable` ProgramData account), or pin an initial admin key. Needs an e2e test. |
| S-2 | Medium (availability) | `process_stake` (~L503–615) | `create_account` for the stake PDA fails if the PDA already holds lamports. Anyone can send 1 lamport to `["staking-stake", victim]`, and the victim's first `Stake` then fails permanently. | If `lamports > 0`, top up with transfer, then `allocate` and `assign` instead of `create_account`. Needs e2e test (CPI path does not run on host). |
| S-3 | Medium (design) | `process_stake` L~600 | `staked_at` is set only when the principal is zero. A top-up does not restart the cooldown, so a deposit made after an old position has matured can be withdrawn immediately with a full `Unstake`. This contradicts a literal reading of INVARIANTS D6 ("cooldown from the last principal change"). | **Decision needed:** either reset `staked_at` on every top-up (matches D6 wording) or document per-first-deposit cooldown and correct D6. |
| S-4 | Low | `process_stake` | `min_stake = 0` permits `amount = 0` deposits, which create a rent-paying account for the caller with no principal. Griefing only. | Reject `amount == 0`. |
| S-5 | Info | `process_unstake` | Reward clamping at `max_supply` silently forfeits rewards (by design, logged via `msg!`). | Keep; state in buyer docs. |

Checked and found correct (no change): signer checks on every mutating instruction,
owner checks, PDA address pinning (config, stake, vault, treasury, metadata),
canonical program-id checks before every `invoke_signed`, checked arithmetic in
fee, reward and cap math, pause does not gate withdrawals, two-step admin
transfer, timelock queue/apply with validation at both points, genesis latch,
one-shot metadata creation, and mint-authority = config PDA.

## Next

Part 2 (`module-sniper`, `module-copy`, `solana-kit`) is next. Decisions needed
before the on-chain fixes S-1 to S-4 are written:
1. S-1: upgrade-authority check, or hardcoded admin?
2. S-3: reset the cooldown on top-up, or document the current behaviour?
3. Should S-2 and S-4 be fixed now without an on-chain test run?
