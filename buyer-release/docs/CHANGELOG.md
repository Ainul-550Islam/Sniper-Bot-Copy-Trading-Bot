# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The canonical version lives in `[workspace.package].version` in the root
`Cargo.toml`; the `VERSION` file mirrors it and `scripts/release-check.sh`
fails the release if they ever disagree.

## [Unreleased]

### Fixed (buyer-hardening pass — 2026-09-18, found by EXECUTING the gated e2e)

- **Staking program, metadata CPI discriminant (real on-chain bug):** the
  hand-rolled `CreateMetadataAccountV3` instruction used discriminant `19`,
  but in the mpl-token-metadata source deployed to mainnet-beta (tag
  `token-metadata@v1.14.0`) variant 19 is `Utilize` and
  `CreateMetadataAccountV3` is variant **33**. The real mainnet-cloned mpl
  program rejected the CPI with `InvalidInstructionData` under
  `solana-test-validator`. Fixed the constant + the pinned byte-layout test;
  verified by the executed e2e (metadata created, replay rejected).
- **validator e2e harness, mpl clone flag:** `--clone <program>` copies only
  the 36-byte program account WITHOUT its programdata account, so the
  upgradeable loader reported "Program is not deployed" at execution time
  (Agave 2.1.21). The harness now passes `--clone-upgradeable-program`.

### Added (buyer-hardening pass)

- `scripts/staking-identity.sh` — program-identity guard: `verify` (declare_id
  vs every tracked reference, placeholder detection), `set-id <keypair>`
  (updates source of truth + buyer-facing docs atomically, re-verifies),
  `deploy --keypair --url` (refuses keypair/declare_id mismatch, refuses the
  placeholder id on public clusters, freshness-checks/rebuilds the .so,
  verifies the on-chain account after deploy). Round-trip tested.
- `docs/LIVE-VALIDATION.md` — operator-controlled live-validation runbook
  (Polymarket live-read checks with raw `eth_call` selectors, in-app
  simulate→live ladder, Solana canary rules, evidence-labeling taxonomy).
- `docs/BUYER-ACCEPTANCE-TEST.md` — 22-step independent buyer acceptance
  procedure.

### Verified by execution (buyer-hardening pass — previously environment-blocked)

- `cargo build-sbf` on the audit-pass source: Agave 2.1.21 / platform-tools
  v1.43 → 187,504-byte `staking_suite.so` (SHA-256 `57a890fa…` after the
  discriminant fix; the pre-fix build `9e113678…` is superseded).
- ALL THREE validator e2e tests EXECUTED and PASSED on a real
  `solana-test-validator` (3/3, 160.72 s batch, `--test-threads=1`).
- pg_dump→restore round-trip re-executed on PostgreSQL 17.11 (dump SHA-256
  `5989ecf1…`; tables/migrations/rowcounts identical; db_integration 23/23
  against the restored DB; application startup + health/ready + audit-verify
  + graceful SIGTERM shutdown against the restored DB).
- Latency benches executed against public devnet (read-only + simulate;
  getSlot p50 65 ms, getLatestBlockhash p50 65 ms, simulateTransaction p50
  66 ms from the sandbox — see `evidence/benchmarks-2026-09-18.json` in the
  release package; NOT product performance claims).

### Fixed (audit pass — live/paper money separation)

- **Module 3 (Polymarket) live sizing balance** — `available_usdc` silently
  returned the cached dashboard balance (which a paper start seeds with a
  1,000 USDC demo figure that survives a runtime mode switch) and otherwise
  fell back to the paper figure **in live mode**. Replaced by
  `available_collateral`: LIVE entries now require a verified on-chain read
  of the funder's collateral (new `collateral.rs` ERC-20 client:
  `balanceOf`/`decimals`/`allowance` via `[polymarket].ctf_rpc_url` against
  `collateral_address`, 15 s freshness bound, decimals plausibility check).
  Unverifiable balances REJECT the entry with typed errors
  (`BalanceUnavailable` / `InsufficientFunding`) — no fallback exists. Live
  orders additionally verify funding and (for EOA signing) the settling
  exchange's ERC-20 allowance before broadcast. Paper/simulate keep the
  demo figure, and only there. Regression tests pin the separation matrix
  (poisoned cache seed, failed/missing/implausible reads).
- **Module 1 (Sniper) `available_sol`** — on RPC failure the cached balance
  was used as a fallback **regardless of execution mode** (contradicting its
  own comment); after a paper→live mode switch a stale/demo seed could size
  live orders. The fallback is now paper-mode-only; simulate/live propagate
  the RPC error into a risk rejection. Unit-tested via a pure fallback rule.
- Module 2 (Copy) audited: already correct (paper cache in paper mode only;
  real RPC read otherwise) — unchanged.

### Added (audit pass — staking program: max supply + token metadata)

- **Immutable max-supply cap** — `Initialize` gained a required `max_supply`
  parameter (> 0, stored in `Config`, deliberately NOT changeable via
  `UpdateParams`). `GenesisMint` now fails with `MaxSupplyExceeded` (6028)
  unless the LIVE mint supply plus the amount stays at or below the cap
  (checked arithmetic; overflow fails closed). Reward minting
  (`Claim`/`Unstake`) is clamped to the remaining headroom so withdrawals
  can never fail because of the cap; the shortfall is forfeited and logged
  on-chain. New error `InvalidMaxSupply` (6029) for a zero cap.
- **Token metadata** — new one-shot admin instruction
  `CreateTokenMetadata{name,symbol,uri}` performing a hand-rolled borsh CPI
  to mpl-token-metadata `CreateMetadataAccountV3` (discriminant 33 — the
  enum index in the mpl source deployed to mainnet-beta, verified by
  executing the CPI against the real mainnet-cloned mpl program in the
  validator e2e — pinned by a byte-layout test): immutable metadata
  (`is_mutable = false`), config
  PDA as mint/update authority, canonical mpl PDA + program-id validation,
  byte-length limits (32/10/200) enforced before the CPI. New errors:
  `MetadataAlreadyExists` (6030), `InvalidMetadataProgram` (6031),
  `MetadataFieldTooLong` (6032). Client builder `create_token_metadata_ix`.
- Staking host unit tests 48 → 71 (cap boundaries incl. exact-cap and
  one-over, live-supply authority, overflow fail-closed, reward clamping at
  zero/partial headroom, claim-succeeds-at-cap, metadata guards + layout).
  New gated validator e2e `validator_e2e_max_supply_cap_and_metadata`
  (cap + metadata against a mainnet-cloned mpl program; NOT executed in the
  audit sandbox — no build-sbf/validator/internet there).
- New module file `crates/module-polymarket/src/collateral.rs` (ERC-20
  collateral reader + unit conversions with mock-RPC wire tests);
  `ctf.rs` address/word helpers shared crate-internally.
- Docs/config updated to match: `config.toml.example` (collateral/ctf_rpc
  semantics, `token_supply` ↔ on-chain `max_supply` mapping),
  `docs/STAKING.md`, `docs/MODULES.md`, `README.md` launch sequence +
  security model.

### Added (buyer / due-diligence documentation pass — no source code changed)

- 14 buyer-package documents under `docs/`: BUYER-OVERVIEW,
  CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE, IP-COMPONENTS, THIRD-PARTY,
  BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST, RELEASE-NOTES-0.1.0, BUYER-FAQ,
  SCOPE-BOUNDARY, SUPPORT-HANDOVER, BUYER-RISK-REGISTER,
  TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST (index).
- README "Buyer / engineering handover" section; `docs/HANDOVER.md` §1 and
  `release-manifest.json` `docs_count` updated for the new document set
  (13 → 27 files under `docs/`). The frozen engineering tree (commit
  `0e139c3`) is untouched: no Rust, SQL, config or CI file changed.

### Added (final delivery package — documentation + bundle tooling only)

- 9 final-delivery documents under `docs/`: FINAL-DELIVERY (single starting
  point), BUYER-QUICKSTART (18-step walkthrough), TECHNICAL-FACT-SHEET,
  SELLER-FACT-SHEET, SELLING-LISTING-SOURCE (factual listing source
  material — not advertising), DEMO-RUNBOOK (10 deterministic demos),
  EVIDENCE-INDEX (claim → evidence map), REPOSITORY-MAP (annotated tree),
  ARCHIVE-CHECKLIST (seller archive INCLUDE/EXCLUDE spec).
- `scripts/verify-delivery.sh` — fast, fail-closed bundle-integrity check
  (required files, version identity, docs count, hygiene, markdown links,
  invisible characters). Complements `release-check.sh`; builds/tests
  nothing.
- CAPABILITY-MATRIX and BUYER-RISK-REGISTER finalized to the delivery column
  schema; IP-COMPONENTS gained the ownership-transfer checklist;
  DELIVERY-MANIFEST indexes all 23 buyer/delivery docs; README/HANDOVER
  pointers and `release-manifest.json` `docs_count` (27 → 36) updated.
  Still zero changes to Rust, SQL, migrations, configs, Docker or CI files.

## [0.1.0] — initial handover release

First complete, internally verified release of the suite. Delivered state
(full evidence trail in `AUDIT.md`, test inventory in `docs/TESTING.md`):

### Added

- **Module 1 — Sniper** (`module-sniper`): pump.fun launch detection
  (PumpPortal WS, Yellowstone-style Geyser `transactionSubscribe`, poll
  fallback) and entry execution with PumpSwap/Raydium/Jupiter exit routing.
- **Module 2 — Copy trading** (`module-copy`): tracked-wallet mirroring with
  per-wallet rules, sizing, staleness guards and mirrored exits.
- **Module 3 — Polymarket** (`module-polymarket`): Gamma + CLOB REST/WS
  integration with EIP-712 v2 order signing and CTF ERC-1155 balance reads.
- **Module 4 — Staking program** (`programs/staking-suite`): native Solana
  program — reward mint, vault + fee treasury, per-second APY accrual,
  parameter timelock (queue/apply/cancel), two-step admin transfer,
  pause-deposits-only, hard parameter caps, one-time latched `GenesisMint`.
- **Module 5 — Telegram control** (`module-telegram`): deny-by-default RBAC,
  kill switch, module on/off, rate-limited alerts.
- **Control plane** (`server`): Axum REST (23 REST endpoints over 21
  `/api` routes) + WebSocket event feed (`/api/events`) + 4 infra routes —
  28 endpoints documented route-by-route in `docs/API.md` +
  embedded dashboard; liveness/readiness probes; Prometheus metrics with
  bounded labels; request-ID correlation; per-IP and per-principal rate
  limits; refusal to bind non-loopback without API auth.
- **Core** (`bot-core`): typed config with validation and env overrides,
  global risk engine (capacity, exposure, daily-loss auto-disable), OMS state
  machine with idempotency keys, restart-safe dedup (memory/Redis/Postgres),
  hash-chained append-only audit trail, JSONL journal with rotation and
  corrupt-line tolerance, intent journal + startup reconciliation,
  Postgres repositories with 11 forward-only migrations.
- **Distributed execution ownership** (`docs/DISTRIBUTED.md`): one logical
  execution ⇒ at most one active owner ⇒ at most one money-moving submission.
  Claim stores (Postgres authoritative, Redis, memory), leases + epochs +
  fencing, handoff grace for ambiguous outcomes, cross-replica kill-switch /
  module-flag sync, position-book sync, cluster-wide `GlobalRiskOracle`
  (tighten-only), and the append-only `execution_claim_events` lineage table.
- **Solana kit** (`solana-kit`): RPC retry/failover/fan-out, WS supervision
  with resubscribe, account cache (TTL + FIFO bounds), pump/raydium/pumpswap
  instruction builders, transaction executor with simulate-first policy and
  signer registry (multi-signer safe).
- **Operations**: Dockerfile (multi-stage, non-root, healthchecked),
  docker-compose stack (Postgres 16 + Redis 7, healthcheck-gated),
  `.env.template`, single-workflow CI (fmt/clippy `-D warnings`/build/test
  with real service containers, staking `build-sbf` + validator e2e,
  cargo-audit + cargo-deny hard gates, docker image build + smoke test),
  `scripts/release-check.sh` local release gate, machine-readable
  `release-manifest.json`, and thirteen docs under `docs/`.

### Fixed (during the release-engineering pass, pre-tag)

- **Audit chain append serialization** — `AuditRepo::append` previously read
  the chain head with `SELECT … ORDER BY id DESC LIMIT 1 FOR UPDATE`, which
  does not serialize concurrent writers under READ COMMITTED (a blocked
  writer's snapshot never sees the winner's new head row → the chain forks
  and `/api/audit/verify` reports a false break). Appends are now serialized
  by a transaction-scoped advisory lock
  (`pg_advisory_xact_lock(hashtext('audit_events_chain'))`). Regression
  tests: concurrent-append linearization + reordered/missing/duplicate row
  detection (`db_integration`).
- Toolchain-pin drift: the Dockerfile built on `rust:1.82` and the CI
  `program` job on unpinned `stable`, contradicting the `rust-toolchain.toml`
  pin (1.98.1). Both now use 1.98.1; `scripts/release-check.sh` gates the
  three-way consistency.
- Removed the placeholder `repository` URL (`example.com/...`) from the
  workspace manifest; stale test counts and an undocumented route-subset
  table in README corrected.

### Fixed (engineering-freeze pass, pre-tag)

- **Telegram bot-token leak into error strings** — the Bot API embeds the
  token in every request URL and `reqwest::Error`'s `Display` appends
  ` for url (…)` on send errors, so failed Telegram calls put the token into
  tracing logs / audit detail / alert text. Every reqwest error mapping in
  `module-telegram` now strips the URL (`Error::without_url()`); regression
  test `error_strings_never_contain_the_bot_token` exercises all four API
  methods against a closed loopback port and fails if the token ever appears
  in an error string.
- **Unused dependencies removed** (verified zero code references before
  removal, `cargo check` + full gate re-run after): `tokio-util` (core,
  solana-kit, server), `sha3` (module-polymarket — EIP-712 uses
  `tiny-keccak`), `serde_with` (workspace entry no crate referenced). They
  remain in `Cargo.lock` only where still required transitively.
- **Release metadata drift corrected:** control-plane route count and docs
  count in this file now match the source (26 `.route()` registrations /
  28 documented endpoints; 13 docs); `release-manifest.json` added as the
  machine-readable delivery manifest and wired into `release-check.sh`
  (required file + version consistency).

### Verification status at cut

- 521 application workspace tests (incl. 38 gated Postgres/Redis/
  distributed/two-replica integration tests), 48+2 staking host/e2e-gated
  tests — 0 failures; `scripts/release-check.sh` 20/20 gates PASS; fmt, clippy `-D warnings`, cargo-audit, cargo-deny
  clean. Per-pass evidence and the honest NOT-EXECUTED /
  ENVIRONMENT-BLOCKED list: `docs/HANDOVER.md` and `docs/TESTING.md`.
- The staking program has **not** had an external security audit; the
  declared program id is a pre-deploy placeholder. Do not deploy to mainnet
  until an independent audit passes (see `docs/SECURITY.md`).

[0.1.0]: initial release — no previous tags exist.
