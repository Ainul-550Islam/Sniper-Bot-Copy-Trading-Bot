# IP / component inventory

Technically significant implementation work in sniper-suite 0.1.0, with
provenance classification. Two categories are used honestly:

- **Original application code** — written for this project; copyright
  transfers with the repository (MIT, see `LICENSE`; holder placeholder is a
  documented handover action).
- **External protocol integration** — original code that *implements against*
  a third-party protocol/API/spec. The code is original; the protocol, its
  programs, APIs, and trademarks are **not** owned by this project and are not
  transferred.

No item below claims ownership of any third-party protocol, SDK, API, or
standard.

## 1. Solana instruction builders (pump.fun / PumpSwap / Raydium / Jupiter)

- **Location:** `crates/solana-kit/src/pump.rs`, `pumpswap.rs`, `raydium.rs`,
  `jupiter.rs`, `layout.rs`, `consts.rs`, `tokens.rs`
- **Purpose:** Hand-rolled account-layout parsing and instruction
  construction for pump.fun bonding-curve buy/sell (incl. v2 variants),
  PumpSwap AMM buy/sell, Raydium AMM `SwapBaseIn`/`SwapBaseInV2`, and Jupiter
  exit routing; SPL token / ATA handling.
- **Dependencies:** `solana-sdk`, `solana-program`, `spl-token`,
  `spl-associated-token-account`, `bincode`, `bs58`.
- **Classification:** Original application code implementing against external
  on-chain protocols (pump.fun, PumpSwap, Raydium, Jupiter). Those protocols,
  their deployed programs and IDs belong to their respective operators;
  on-chain program addresses are facts, not IP.
- **Third-party licensing:** Only via the crates listed (see
  `docs/THIRD-PARTY.md`). No protocol SDK is vendored.

## 2. Transaction decoder / swap decoding

- **Location:** `crates/solana-kit/src/decode.rs`
- **Purpose:** Decodes executed transactions (wire + parsed forms) into typed
  swap/buy/sell events used by sniper exits, copy mirroring, attribution and
  reconciliation; handles versioned transactions and failed-tx skipping.
- **Dependencies:** `solana-sdk`, `solana-transaction-status`,
  `solana-account-decoder`.
- **Classification:** Original application code.

## 3. Execution engine

- **Location:** `crates/solana-kit/src/execute.rs`, `tx.rs`, `rpc.rs`,
  `ws.rs`
- **Purpose:** Transaction assembly, blockhash management, simulate-first
  policy (`SIMULATE_FIRST` / `ABORT_ON_SIMULATION_FAILURE`), retry/failover
  RPC chokepoint with bounded consecutive-failure tracking, optional
  broadcast fan-out race across primary+fallback RPCs, WebSocket supervision
  with resubscribe.
- **Dependencies:** `solana-sdk`, `solana-client`, `reqwest`,
  `tokio-tungstenite`, `ed25519-dalek`.
- **Classification:** Original application code.

## 4. Signer abstraction / signer registry

- **Location:** `crates/solana-kit/src/signer.rs`
- **Purpose:** `TransactionSigner` trait + named `SignerRegistry`
  (`primary_trading` + configured identities); enforces that every required
  signer of a multi-signer transaction is declared and resolvable (structured
  failure otherwise); custody-backend selection (`local` implemented;
  `vault`/`kms`/`hsm` fail startup — no silent fallback). Key material never
  reaches trading modules.
- **Dependencies:** `solana-sdk`, `ed25519-dalek`, `async-trait`.
- **Classification:** Original application code.

## 5. Risk engine + global risk oracle

- **Location:** `crates/core/src/risk.rs`; cluster oracle in
  `crates/core/src/ownership.rs` and `docs/DISTRIBUTED.md`
- **Purpose:** Pre-trade global checks (capacity, exposure, daily-loss
  auto-disable), risk-rejection events/metrics, tighten-only cluster-wide
  limit propagation (`GlobalRiskOracle`).
- **Dependencies:** none beyond `bot-core` internals.
- **Classification:** Original application code.

## 6. OMS (order management + idempotency)

- **Location:** `crates/core/src/oms.rs`, `dedup.rs`, `models.rs`
- **Purpose:** Order state machine with idempotency keys; restart-safe dedup
  across memory / Redis / Postgres levels; ambiguity-aware status handling
  (e.g. simulate-mode outcomes never masquerade as confirmed).
- **Classification:** Original application code.

## 7. Reconciliation + recovery

- **Location:** `crates/core/src/reconciliation.rs`, `recovery.rs`;
  `crates/server/src/recon.rs`; migrations `0005`, `0006`, `0007`, `0008`
- **Purpose:** Intent journal (record-before-send), startup replay,
  venue-truth resolution of unresolved intents, transaction attribution for
  unknown on-chain transactions, handoff grace for ambiguous outcomes, PnL
  replay rules (`docs/RECONCILIATION.md`).
- **Classification:** Original application code.

## 8. Distributed ownership (claims / leases / epochs / fencing)

- **Location:** `crates/core/src/ownership.rs`, `redis_ownership.rs`,
  `db/claims.rs`; migrations `0009`, `0010`, `0011`; `docs/DISTRIBUTED.md`
- **Purpose:** The invariant *one logical execution ⇒ ≤1 active owner ⇒ ≤1
  money-moving submission*: claim stores (Postgres authoritative, Redis,
  memory), leases + epochs + fencing tokens, handoff grace, cross-replica
  kill-switch/module-flag sync, position-book sync, append-only
  `execution_claim_events` lineage.
- **Classification:** Original application code (the design pattern of
  leases/fencing is general distributed-systems practice; this is an
  independent implementation, not derived from a specific third-party
  codebase).

## 9. Audit chain

- **Location:** `crates/core/src/audit.rs`; append serialization in
  `crates/core/src/db/repo.rs`; migration `0004`
- **Purpose:** Append-only, hash-chained audit trail; app APIs can never
  mutate or delete entries; `GET /api/audit/verify` re-computes the chain;
  advisory-lock serialized appends keep the chain linear under concurrency.
- **Classification:** Original application code (hash-chain audit logs are a
  standard technique; implementation is independent).

## 10. Geyser integration + account cache

- **Location:** `crates/solana-kit/src/events.rs` (Yellowstone-style
  `transactionSubscribe` client), `cache.rs` (TTL + FIFO-bounded warm account
  cache), `pumpportal.rs` (PumpPortal WS client)
- **Purpose:** Push-based launch/trade detection with poll fallback; warm
  caching of semi-static accounts to cut RPC latency/load.
- **Classification:** Original application code implementing against the
  Yellowstone gRPC/WS `transactionSubscribe` convention and the PumpPortal
  API — both external services; neither is owned by this project. A
  compatible Geyser provider and (optionally) PumpPortal access are
  buyer-contracted external services.

## 11. Polymarket EIP-712 / CLOB implementation

- **Location:** `crates/module-polymarket/src/eip712.rs` (v2 11-field `Order`
  struct hash, domain `Polymarket CTF Exchange` v2, chainId 137, type-3
  signature wrapping), `orders.rs`, `clob.rs`, `gamma.rs`, `auth.rs` (L1/L2
  auth headers), `ctf.rs` (ERC-1155 balance reads), `ws.rs`, `strategy.rs`
- **Purpose:** Complete client-side implementation of Polymarket's CLOB
  order signing and trading APIs.
- **Dependencies:** `k256`, `tiny-keccak`, `hmac`, `sha2`, `reqwest`,
  `tokio-tungstenite`, `num-bigint`.
- **Classification:** Original application code implementing against
  Polymarket's published API and the EIP-712 **standard** (Ethereum
  improvement proposal — a public specification, not owned by anyone here).
  Polymarket's contracts, APIs, exchange addresses and brand belong to
  Polymarket; nothing here grants rights to operate on their venue beyond
  their own terms of service, which the buyer must satisfy.

## 12. Telegram control plane

- **Location:** `crates/module-telegram/src/` (commands, RBAC, alerts,
  token-redacted Bot API client)
- **Purpose:** Deny-by-default remote control (owner/operator/readonly),
  kill switch, module toggles, rate-limited alerts.
- **Classification:** Original application code implementing against the
  Telegram Bot API (external service; bot token and Telegram ToS are
  buyer-side).

## 13. Staking program (on-chain)

- **Location:** `programs/staking-suite/src/{lib,processor,state,instruction,error}.rs`
- **Purpose:** Native Solana program (no Anchor): reward mint (authority =
  config PDA), staking vault + fee treasury ATAs, deposit fee, per-second APY
  accrual, hard parameter caps, queue/apply/cancel timelock, two-step admin
  transfer, pause-deposits-only, one-shot latched `GenesisMint`.
- **Dependencies:** `solana-program` 2.1, `spl-token`, `spl-associated-token-account`,
  `borsh`, `thiserror`, `solana-system-interface`.
- **Classification:** Original application code. **Caveats that transfer with
  it:** not externally audited; declared program id is a pre-deploy
  placeholder; mainnet deployment is documentation-blocked pending an
  independent audit.

## 14. Persistence model

- **Location:** `crates/core/src/db/` (sqlx repositories), migrations
  `0001`–`0011`, `storage.rs` (JSONL journal), `redis_kv.rs`
- **Purpose:** PostgreSQL as durable financial truth (orders, executions,
  positions, trades, intents, claims, flags, audit), forward-only migration
  discipline, crash-tolerant local journal, Redis strictly non-authoritative.
- **Classification:** Original application code (schema, queries, journal
  format all project-specific).

## 15. Control plane, observability, testing infrastructure

- **Location:** `crates/server/src/` (API, WS feed, dashboard, probes,
  metrics, persist/recon pumps), `crates/core/src/obs/`, all `tests/`
  directories, `scripts/release-check.sh`, `.github/workflows/ci.yml`,
  `deny.toml`
- **Purpose:** 28-endpoint control plane with RBAC/rate limits/correlation
  IDs; bounded-label Prometheus metrics; protocol-mock test harnesses
  (PumpPortal WS, Geyser WS, JSON-RPC pair, CLOB/Gamma HTTP); one-command
  release gate; CI and supply-chain policy.
- **Classification:** Original application code (the embedded dashboard is a
  single original HTML file served by Axum; no frontend framework).

## Summary of what is **not** transferred / not owned

- pump.fun, PumpSwap, Raydium, Jupiter, Solana, PumpPortal, Yellowstone,
  Polymarket (contracts, APIs, addresses, brands), Telegram (Bot API),
  PostgreSQL, Redis, Docker — all third-party; this project integrates with
  them under their own terms.
- Rust crate dependencies — licensed per `Cargo.lock` + `deny.toml` policy
  (see `docs/THIRD-PARTY.md`); MIT/Apache-2.0-style licenses permit
  redistribution with notices, which the lockfile + deny tooling document.
- The EIP-712 and borsh formats are public standards/specifications.

## Ownership transfer checklist

Every item that must change hands (or be created) for the buyer to own and
operate the system outright. No credentials or identities are invented here;
each line names the artifact and its state at delivery. Procedural detail:
`docs/SUPPORT-HANDOVER.md`; sign-off format: `docs/ACCEPTANCE-CHECKLIST.md`.

- [ ] **Repository ownership** — the authoritative git repository (history
      `9c677cd` → `0e139c3` + documentation commits) transferred via bundle
      or hosting transfer (`docs/ARCHIVE-CHECKLIST.md`); buyer verifies per
      `docs/BUYER-QUICKSTART.md` §1.
- [ ] **Git hosting ownership** — remote/hosting account (GitHub org or
      equivalent) created or transferred by the buyer; set the real
      `repository` URL in the workspace `Cargo.toml` at that point
      (deliberately absent until then).
- [ ] **License holder** — insert the legal copyright entity into `LICENSE`
      (currently the documented placeholder "sniper-suite authors"); execute
      any IP assignment paperwork between the parties.
- [ ] **Staking program authority** — deploy-keypair custody decided;
      program deployed under the finalized id; `Initialize` called with the
      admin set to the buyer's **multisig PDA** (Squads/Realms recommended);
      timelock ≥ 24h; one-shot `GenesisMint` recipient and distribution plan
      owned by the buyer (`docs/STAKING.md`).
- [ ] **Deployment credentials** — host/container credentials, SSH keys,
      orchestrator tokens: created by the buyer; nothing ships.
- [ ] **RPC provider accounts** — Solana RPC/WS (and failover) provider
      contracts in the buyer's name; API keys issued by the provider.
- [ ] **Geyser / PumpPortal accounts** — optional feed-provider contracts in
      the buyer's name (poll fallback exists without them).
- [ ] **Polymarket credentials** — Polymarket API access + Polygon key
      generated and held by the buyer; buyer satisfies Polymarket ToS and
      applicable law.
- [ ] **Telegram bot ownership** — bot created (or transferred) via
      BotFather under the buyer's account; token rotated; allow-lists set to
      the buyer's owner/operator/readonly ids.
- [ ] **Monitoring ownership** — Prometheus/log/alerting stack stood up by
      the buyer; scrape target `/metrics`; runbook `docs/OPERATIONS.md`.
- [ ] **Domain ownership** — any domains/reverse-proxy/TLS certificates for
      API exposure: registered and held by the buyer (the server refuses
      non-loopback binds without API auth until then).
- [ ] **CI secrets** — GitHub (or equivalent) environment secrets/tokens for
      CI runs: created by the buyer in their own CI environment.
- [ ] **Docker registry ownership** — image registry namespace/account:
      buyer's; images are built by the buyer from the delivered Dockerfile
      (no images ship).
- [ ] **Backup ownership** — backup destination, schedule and restore
      responsibility: buyer's, per `docs/BACKUP-RESTORE.md`; perform one
      dump→restore drill at acceptance.
- [ ] **Credential rotation** — every credential that ever existed on either
      side rotated at transfer (`docs/SUPPORT-HANDOVER.md` §6), regardless of
      the clean secret-scan evidence.
