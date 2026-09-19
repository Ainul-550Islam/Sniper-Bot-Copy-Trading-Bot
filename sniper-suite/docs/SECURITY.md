# Security model

This document describes the security controls that are **implemented and
tested** in this repository. It is not an external audit — no third-party
audit has been performed (see AUDIT.md for the honest status of assurance
claims).

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
* **Key custody providers:** `[signing] provider` = `local` (implemented) |
  `vault` | `kms` | `hsm` (**not implemented in this build** — selecting one
  fails startup with `SignerError::UnsupportedBackend`; there is no silent
  fallback to local keys). Adding a backend means implementing
  `TransactionSigner` and extending `build_signer_registry`; no business
  logic changes. Polymarket EVM signing (secp256k1/EIP-712) is deliberately
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
* **Polymarket:** private key + funder via env-seeded config; EIP-712
  signing happens in-process.

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
