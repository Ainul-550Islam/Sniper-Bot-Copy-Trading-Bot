# Security model

This document describes the security controls that are **implemented and
tested** in this repository. It is not an external audit — no third-party
audit has been performed (see AUDIT.md for the honest status of assurance
claims; a future audit's deliverables have a defined handover slot in
`docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006).

### Frontend dependency advisories (remediated 2026-09-27)

Two batches of remediation, both verified in this repository's own gates:

* **Batch 10** — `apps/control-plane` pinned `next 15.5.4`, affected by the CVSS 10.0
  remote-code-execution advisory `CVE-2025-66478` (React Server Components / App Router, upstream
  `CVE-2025-55182`). The pin was raised to the patched `next 15.5.26` for the 15.5 line (superseded the same day by Batch 11 — see below).
* **Batch 11 (F-2)** — the 15.5 line still resolved `postcss 8.4.31`, which `npm audit` rates high
  (unescaped `</style>` output; `sourceMappingURL` `.map` disclosure) and whose fix ships only in
  `next >= 16.3.6`. The control plane now pins **`next 16.3.6`** with **`eslint-config-next 16.3.6`**,
  which resolves **`postcss 8.5.23`**; the shipped dependency graph now audits clean (`0 vulnerabilities`).

Verification for the current state (`npm ci` = clean dependency state):

* `npm ci --ignore-scripts` → 354 packages, exit 0; lockfile/package.json consistency check as in
  `.github/workflows/frontend-ci.yml` → `lockfile consistent`.
* `npm audit` → **`found 0 vulnerabilities`** (critical 0, high 0, moderate 0, low 0).
* `npx tsc --noEmit` → exit 0 · `npx next build` → exit 0 (Next.js 16, 5 routes prerendered static).
* `npm run lint` → exit 0, non-interactive (ESLint 9 flat config, `eslint-config-next/core-web-vitals`
  + `/typescript`); the 12 pre-existing findings are reported as warnings and enumerated in
  `docs/KNOWN-LIMITATIONS.md` row 16 — no rule is disabled.
* Regression guard: `release_manifest_integration::frontend_lockfile_pins_patched_nextjs` fails if the
  shipped tree is reverted to `next 15.5.26` / `15.5.4` or to a `postcss < 8.5.23` resolution.

### Six tracked gaps — single source of truth (never inferred from hermetic tests)

Source: `crates/server/src/ops/final_gap_ledger.rs` + `docs/FINAL-BUYER-GAP-LEDGER.md`; live evidence records:
`evidence/external/*.json` (`NOT_RUN` in hermetic). None of the six is `VERIFIED`; the registry promotes only via
`mark_verified(id, evidence_ref, verified_at, detail)` after the real command ran in the required environment.
Documented commands are asserted to match the executable harnesses by the test `ledger_commands_match_documented_harnesses`.

| Gap | Area | Ledger status (hermetic) | Buyer/operator command or deliverable |
|---|---|---|---|
| GAP-001 | Live billing (Stripe / Paddle) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test -p sniper-suite --test live_billing_contract -- --ignored --nocapture` (Paddle: `PADDLE_API_KEY=...`) |
| GAP-002 | Remote custody (Vault / KMS / HSM) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `LIVE_CUSTODY=1 VAULT_ADDR=... VAULT_TOKEN=... cargo test -p sniper-suite --test live_custody_contract -- --ignored --nocapture` |
| GAP-003 | Deployment smoke (staging / production) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `DEPLOYMENT_BASE_URL=https://<real> cargo test -p sniper-suite --test deployment_smoke -- --nocapture` (missing URL ⇒ fail safe, no `localhost` substitution) |
| GAP-004 | Funded live trading transition | `EXTERNAL_REQUIRED` / operator-only | Guard evidence: `cargo test -p sniper-suite --lib funded_mode_guard`; funded result only from a supervised operator run |
| GAP-005 | Staking validator E2E | `EXTERNAL_REQUIRED` / `NOT_RUN` | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` |
| GAP-006 | External security audit | `EXTERNAL_REQUIRED` / `BUYER_ACTION` — no report exists | Handover slot: `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006 (findings/severity/remediation/retest/sign-off) |

All live modes: `bash scripts/run-external-validation.sh all-safe` → `6/6 NOT_RUN` in a credential-free sandbox (correct, not a failure).

## Threat model (summary)

| Threat | Control |
|---|---|
| Runaway trading / bad signal | Global `RiskEngine` gate before ANY execution: kill switch, per-trade max, daily loss/drawdown breaker, open-exposure cap. Modules cannot bypass it (they hold no direct executor path). |
| Rogue operator / stolen API key | Role-based keys (readonly/operator/owner), digest-only storage, per-IP rate limiting, every mutation + denial audited. Live-mode switch requires `owner`. |
| Accidental internet exposure | Fail-closed bind: non-loopback `[api] bind` without configured auth refuses to start. Compose publishes on 127.0.0.1 by default. |
| Duplicate execution (feed replay, WS reconnect) | Two-layer dedup: in-process bounded L1 + durable L2 (Postgres unique constraint or Redis SET NX with TTL). First-arrival-wins; L2 outage degrades to L1 verdict + metric, never to double-spend silence. |
| Lost state / crash mid-flight | OMS idempotency keys persist intents; startup recovery reloads positions + unfinished orders (as `Unknown`) and sweeps unresolved transactions; recon queue retries with backoff against on-chain/venue truth. |
| Tampered history | Audit trail is a sha256 hash chain (genesis row; appends serialized by a Postgres advisory lock). `/api/audit/verify` detects any modified/removed row. App APIs cannot update or delete audit rows. |
| Secret leakage | Keys/secrets never logged or exposed via metrics/health/config endpoint (config view redacts; API keys stored as sha256 digests; wallet keypair read from file/env at startup only). |
| Admin abuse (staking program) | Parameter changes go through a public timelock (`UpdateParams` → wait → permissionless `ApplyParams`, with hard caps: fee ≤ 1000 bps, reward ≤ 10000 bps, timelock ≤ 30 days). Admin transfer is two-step. Pause can never block withdrawals. Genesis mint is admin-only and latched to execute at most once. |

## Key management

* **Solana wallet:** loaded from `SOLANA_KEYPAIR` (path / base58 / JSON
  array). The file is mounted read-only in compose and gitignored. There is
  no API that returns or re-exposes the secret.
* **Signer abstraction (key-custody boundary):** trading modules never touch
  key material. `solana-kit/src/signer.rs` defines `TransactionSigner`
  (async `pubkey()` / `sign_message()` / `sign_versioned_message()`, `Debug`
  is a secret-free supertrait) and a `SignerRegistry` mapping logical
  identities (`primary_trading` — always the loaded wallet — plus configured
  names such as `sniper`, `copy_trading`, `treasury`, `staking_admin`) to
  signers. Lookups are deterministic and fail closed: an unknown identity or
  an unresolvable required signer is a structured `SignerError`, never a
  silent fallback to another wallet. `Wallet` exposes no keypair accessor;
  `sign_message_sync` is the single local signing choke point.
* **Multi-signer transactions:** `TxRequest::extra_signers` is a hard
  contract — the compiled message's required-signer set must exactly equal
  {wallet} ∪ dedup(extra_signers), every extra must resolve through the
  registry, and signatures are collected in message order. Mismatches,
  missing signers and backend failures abort the build (`SignerError::
  SignerMismatch / MissingSigner / ExtraSignerNotRequired / SigningFailed`).
* **Key custody providers — two layers, both fail-closed:**
  * *Transaction-signer layer* (`[signing] provider` →
    `build_signer_registry`): `local` implemented; `vault`/`kms`/`hsm`
    fail startup with `SignerError::UnsupportedBackend` — there is no
    silent fallback to local keys. Adding a backend means implementing
    `TransactionSigner` and extending `build_signer_registry`; no business
    logic changes.
  * *Multi-tenant custody boundary* (`CUSTODY_PROVIDER=local|vault|kms|hsm`
    → `crates/server/src/custody/provider_registry.rs`): `vault` and `kms`
    are REAL adapters — Vault transit engine (REST `transit/sign`,
    redacted token) and AWS KMS (hand-rolled SigV4 verified against the
    AWS-documented test vector, Ed25519 `EDDSA_SHA_512`) — unit-tested,
    fail-closed, never live-proven in this workspace; `hsm` refuses with
    its exact PKCS#11 dependency. Remote activation additionally requires
    `LIVE_CUSTODY=1` (no silent remote activation). Current state:
    `docs/CUSTODY-STATUS-2026.md`.
  * Polymarket EVM signing (secp256k1/EIP-712) is deliberately
    separate and not routed through this Solana abstraction.
* **Redaction:** `SecretConfig` has a hand-written `Debug` emitting only
  `<set>`/`<unset>`; `Wallet` and `LocalKeypairSigner` `Debug` print public
  keys and load-source only; `/api/config` and the recorded config version
  replace `secrets` with `<redacted>`; signer errors carry identities,
  pubkeys and context strings only — never key bytes or specs.
* **API keys:** declared as `[auth] [[auth.keys]]` `{label, key_env, role}`
  principals — plaintext is read once from the environment and only ever
  handled as a sha256 digest afterwards (memory + `api_keys` table).
  Runtime-added keys (`POST /api/keys`, owner-only, ≥24 chars) follow the
  same digest-only rule. `/api/keys` lists digests and last-used timestamps
  — sufficient to audit, useless to replay.
* **Telegram:** allowlists by user/chat id; bot token via
  `TELEGRAM_BOT_TOKEN` env (not the config file). The Bot API embeds the
  token in every request URL, so all `module-telegram` error mappings strip
  the URL from `reqwest` errors (`Error::without_url()`) before the message
  can reach logs, audit detail or alert text — regression-tested by
  `error_strings_never_contain_the_bot_token`.
* **Endpoint URLs:** RPC/WS endpoint URLs are logged on connect and may
  appear in wrapped transport errors (they identify the dependency being
  diagnosed). If your provider puts an API key in the URL query/path, treat
  those log lines as secret-bearing: prefer providers that authenticate by
  header, or restrict log access accordingly. The suite's own secrets are
  never part of any URL it logs.
* **Polymarket:** private key (`POLYMARKET_PRIVATE_KEY` / `POLYGON_PRIVATE_KEY`)
  + funder via env-seeded config; EIP-712 signing happens in-process. The
  CLOB L2 credentials (`POLY_API_KEY` / `POLY_API_SECRET` /
  `POLY_API_PASSPHRASE`) are read from the environment or derived from the
  key at start-up; they are used for the HMAC request headers and the
  authenticated user-websocket frame and are never logged, journaled or
  audited (the journal stores order ids and sizes only).

## Execution safety

* Default mode is **paper**. Broadcasting requires BOTH
  `[execution] mode = "live"` AND `allow_live_trading = true`.
* `simulate_first` runs a simulation before live broadcast where the venue
  supports it (disabled automatically in pure paper mode).
* Kill switch: `/api/kill`, `/kill` on Telegram, or risk-breaker trip stops
  new intent acceptance immediately; exit handling continues so positions are
  not stranded.

## Dependency & supply chain

* `cargo-audit` (app + program lockfiles) and `cargo-deny`
  (advisories/bans/sources blocking; licenses reported) run in CI on every
  push. `rust-toolchain.toml` pins the compiler; the program's Cargo.lock is
  pinned to the solana 2.1 generation required for BPF compilation.
* No `unsafe` in application crates; the on-chain program uses only the
  documented solana-program CPI surface.

## Known limitations (honest list)

* No third-party security audit of the staking program or the bot.
* The hash chain protects against app-level tampering; an attacker with
  direct DB access can rewrite the chain consistently (defense requires DB
  credentials hygiene / row-level security at the DB tier).
* Rate limiting is per-process (in-memory buckets); multi-instance
  deployments would need a shared limiter.
* Withdrawal-from-vesting style protections (e.g. multi-sig admin) are not
  implemented — a single admin key controls the staking program within the
  timelock constraints.
