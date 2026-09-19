# Commercial scope boundary

What transfers with sniper-suite 0.1.0, and what does not. Four categories;
every item is factual. This document exists so that neither party has to
guess where the software ends.

## 1. DELIVERED SOFTWARE (transfers with the repository)

- **Rust application source** — 7-crate workspace: `bot-core`, `solana-kit`,
  `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`,
  `sniper-suite` server binary (`crates/`).
- **Staking program source** — `programs/staking-suite` (native Solana
  program, own lockfile; BPF build PREVIOUSLY VERIFIED, not deployed).
- **Tests** — 521 workspace tests (incl. 38 gated integration tests),
  48 + 2 staking host/e2e tests, all mock harnesses (`tests/` dirs across
  crates).
- **Database migrations** — 11 forward-only PostgreSQL migrations
  (`crates/core/migrations/`).
- **Deployment configuration** — `Dockerfile`, `docker-compose.yml`,
  `.dockerignore`, `.env.template`, `config.toml.example`,
  `rust-toolchain.toml`, `deny.toml`, `.gitignore`.
- **CI** — `.github/workflows/ci.yml` (4 jobs; requires the buyer's runner to
  execute).
- **Release tooling** — `scripts/release-check.sh` (20-gate local release
  validation), `release-manifest.json`.
- **Documentation** — 13 engineering docs (`docs/`: ARCHITECTURE, API,
  SECURITY, DEPLOYMENT, OPERATIONS, MODULES, STAKING, TESTING,
  RECONCILIATION, DISTRIBUTED, RELEASE, HANDOVER, BACKUP-RESTORE) + 14
  buyer-package docs (this set; index in `docs/DELIVERY-MANIFEST.md`) +
  README, CHANGELOG, AUDIT.md (historical evidence trail), root SECURITY.md,
  LICENSE (MIT, holder placeholder), VERSION.

**Not delivered inside this category:** no compiled binaries, no Docker
images, no deployed on-chain program, no populated databases — the buyer
builds everything from source (procedure: `docs/BUYER-DEPLOYMENT.md`).

## 2. BUYER-PROVIDED INFRASTRUCTURE (buyer must supply & operate)

- Production **PostgreSQL ≥ 16** instance (+ backups per
  `docs/BACKUP-RESTORE.md`).
- Production **Redis 7** instance.
- Hosting for the bot process(es)/containers (single or multi-replica),
  including any orchestrator.
- **Monitoring stack** — Prometheus (or compatible) scraping `/metrics`,
  log pipeline consuming the JSON log format, alert routing per
  `docs/OPERATIONS.md`.
- **Secret store / env injection** for keys and tokens (the software reads
  secrets from the environment only).
- **Domains / reverse proxy / TLS** if the API is exposed beyond loopback
  (the server refuses non-loopback binding without API auth; exposing it
  publicly is the buyer's decision and responsibility).
- **CI runners** (GitHub Actions or an adapted equivalent).
- **Funded trading keys** and the capital-risk policy around them.

## 3. EXTERNAL SERVICES (third parties; buyer contracts & pays)

- **Solana RPC + WebSocket providers** (mainnet/devnet access, rate limits,
  failover endpoints).
- **Geyser provider** (Yellowstone-compatible `transactionSubscribe`) —
  optional; poll fallback exists.
- **PumpPortal API** — optional feed for pump.fun launch/trade detection.
- **Polymarket** — Gamma + CLOB APIs and the Polygon-hosted CTF Exchange
  contracts; buyer must satisfy Polymarket's terms of service and applicable
  law in their jurisdiction.
- **Telegram** — Bot API access and the buyer's own bot (BotFather token).
- **Cloud/infrastructure vendors** of the buyer's choice.
- The on-chain protocols themselves (pump.fun, PumpSwap, Raydium, Jupiter,
  Solana) are public infrastructure — no contract needed, but their programs
  can change without notice (`docs/BUYER-RISK-REGISTER.md`).

## 4. HUMAN / LEGAL RESPONSIBILITIES (not software; not transferable by code)

- **License ownership & copyright assignment** — inserting the legal entity
  into `LICENSE`; any IP assignment agreement between the parties.
- **Regulatory review** — trading crypto-assets and prediction markets is
  regulated unevenly across jurisdictions; compliance (including whether
  Polymarket access is lawful for the buyer) is solely the buyer's
  responsibility (README §Disclaimer).
- **External security audit** — none exists; commissioning one (especially
  before any staking-program mainnet deployment) is a buyer decision/cost.
- **Custody policy** — how keys are generated, stored, rotated, and who may
  use them; the software provides the signer boundary, not the policy.
- **Live-trading approval & supervision** — flipping both live gates, sizing
  risk limits, gradual funded validation, and kill-switch drills are operator
  acts (`docs/BUYER-DEPLOYMENT.md` §15).
- **Security contact & incident response ownership** — publishing a real
  contact in `SECURITY.md` and staffing it (`docs/SUPPORT-HANDOVER.md`).
- **Venue terms of service** — accepting/complying with the ToS of every
  venue connected.

## Boundary rules of thumb

1. If it is a file in this repository, it is delivered software (category 1).
2. If it must run somewhere the seller does not control, it is buyer
   infrastructure (category 2) or an external service (category 3).
3. If it requires a human decision, signature, payment, or legal judgment,
   it is category 4 — no code in this repository makes those decisions, and
   the software deliberately blocks several of them behind explicit gates
   (live mode, mainnet staking deployment).
