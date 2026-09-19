# Engineering handover

This document lets a receiving engineering team verify, run and maintain the
repository from a cold machine. It states exactly what was verified where,
and what was not.

## 1. What is being handed over

The complete source of a modular crypto trading system (5 modules + control
plane) at version `0.1.0` (see `VERSION`, `CHANGELOG.md`):

- `crates/` — 7-crate cargo workspace (bot-core, solana-kit, module-sniper,
  module-copy, module-polymarket, module-telegram, server).
- `programs/staking-suite/` — standalone native Solana program (own
  lockfile; built with `cargo build-sbf`, agave 2.1.21).
- `crates/core/migrations/` — 11 forward-only Postgres migrations (embedded
  in the binary; applied at startup when `auto_migrate` is on).
- `docs/` — 13 engineering documents: ARCHITECTURE, API, SECURITY,
  DEPLOYMENT, OPERATIONS, MODULES, STAKING, TESTING, RECONCILIATION,
  DISTRIBUTED, RELEASE, HANDOVER (this file), BACKUP-RESTORE — plus 23
  buyer/delivery documents added after the engineering freeze (buyer
  package: BUYER-OVERVIEW, CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE,
  IP-COMPONENTS, THIRD-PARTY, BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST,
  RELEASE-NOTES-0.1.0, BUYER-FAQ, SCOPE-BOUNDARY, SUPPORT-HANDOVER,
  BUYER-RISK-REGISTER, TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST;
  final delivery package: FINAL-DELIVERY, BUYER-QUICKSTART,
  TECHNICAL-FACT-SHEET, SELLER-FACT-SHEET, SELLING-LISTING-SOURCE,
  DEMO-RUNBOOK, EVIDENCE-INDEX, REPOSITORY-MAP, ARCHIVE-CHECKLIST;
  index: `docs/DELIVERY-MANIFEST.md`, start: `docs/FINAL-DELIVERY.md`).
  No source code changed in those passes; the only executable added is
  `scripts/verify-delivery.sh` (documentation/bundle integrity checker —
  it builds and tests nothing).
- Deployment assets: `Dockerfile`, `docker-compose.yml`, `.dockerignore`,
  `.env.template`, `config.toml.example`, `.github/workflows/ci.yml`,
  `deny.toml`, `rust-toolchain.toml`, `scripts/release-check.sh`,
  `release-manifest.json` (machine-readable delivery manifest).
- `AUDIT.md` — the full historical audit/build trail with per-pass evidence.

No secrets, keys, credentials, private databases or build artifacts are part
of the repository (`.gitignore`/`.dockerignore` enforce; the tree was
scanned at release — see `scripts/release-check.sh`).

## 2. Verifying from zero (exact steps)

Requirements: Linux x86-64, ~2 GB RAM, ~20 GB disk, network access.

```bash
# Toolchain (rustup honors rust-toolchain.toml automatically):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env          # or export PATH="$HOME/.cargo/bin:$PATH"

# Datastores (or use the compose stack / CI service containers):
#   PostgreSQL >= 16 on some port, Redis 7 on some port.
export POSTGRES_URL=postgres://postgres@127.0.0.1:5433/postgres
export REDIS_URL=redis://127.0.0.1:6379

# Full local release gate (fmt, check, clippy -D warnings, workspace tests,
# gated integration suites, staking host tests, audit, deny, consistency):
./scripts/release-check.sh
```

Equivalent manual matrix — the exact commands and their last known results
are in `docs/TESTING.md` §Layers and §"What is covered where".

Reproducibility evidence: the whole suite has been rebuilt and re-verified
**from source alone on a wiped machine** (fresh rustup install, PostgreSQL
16.4 compiled from the official tarball, Redis 7.2.10 compiled from source,
empty target directory, empty database), and re-gated on the final tree by
`scripts/release-check.sh` (**20/20 gates PASS**): workspace 521/521,
db_integration 23/23 (fresh + rerun + `pg_dump`→restore round-trip with the
full suite green on the restored database), redis 10/10, distributed 4/4,
two-replica 1/1, staking 48/48 host (gated e2e skipped — no validator),
fmt/clippy/audit/deny all clean. The earlier 518/518, db 21/21 figures
predate the two audit-chain regression tests added in the release pass
(see CHANGELOG "Fixed"). The post-delivery AUDIT PASS (2026-09-18, this
tree) re-gated everything after the live/paper balance-separation fixes and
the staking max-supply/metadata additions: workspace **537/537** (real
PostgreSQL 17.11 + Redis 8.0.2), staking host **71/71**, fmt/clippy/audit/
deny clean — see `AUDIT.md` §28 and CHANGELOG [Unreleased].

## 3. Verification status taxonomy (honest labeling)

| Label | Meaning |
|---|---|
| VERIFIED | Executed successfully in the most recent full pass in the handover environment (results in `AUDIT.md` final sections + `docs/TESTING.md`) |
| PREVIOUSLY VERIFIED | Executed successfully in an earlier build session on identical source, not re-executed in the latest restored environment |
| GATED | Runs automatically when its env var/dependency is present; skips cleanly otherwise |
| NOT EXECUTED / ENVIRONMENT-BLOCKED | Cannot run in the build sandbox; wired into CI or requires external resources |

Current classification:

- **VERIFIED (latest pass — audit pass 2026-09-18, release gate
  `scripts/release-check.sh` 20/20):** all 537 workspace tests (incl. the
  38 gated integration tests against real PG 17.11 + Redis 8.0.2), 71
  staking host tests, fmt, `clippy -D warnings` (both cargo projects),
  `cargo check`, cargo-audit (both lockfiles), cargo-deny, migrations
  0001–0011 applied on a fresh database. The freeze-gate pass (521/521,
  48 staking host, PG 16.4 + Redis 7.2.10) additionally included a
  `pg_dump`→restore→full-suite round-trip on the restored database.
  The buyer-hardening pass RE-EXECUTED that round-trip in its sandbox
  (PostgreSQL 17.11: dump SHA-256 `5989ecf1…`, restore to a clean database,
  table/rowcount/schema identity, db_integration 23/23 ON the restored DB,
  plus app startup + health/ready/status/metrics + clean SIGTERM shutdown
  against it — `evidence/phase8-*`).
- **VERIFIED BY EXECUTION (buyer-hardening pass, 2026-09-18, agave 2.1.21 +
  platform-tools v1.43):** `cargo build-sbf` (187,504-byte .so, SHA-256
  `57a890fa…`; byte-identical rebuild from the same source — real
  reproducibility evidence); the `STAKING_E2E=1` validator e2e — **all 3
  tests executed + passed** (160.72 s, Agave BPF VM, real mpl-token-metadata
  clone from mainnet-beta); `latency_bench` read-only + simulate legs against
  public devnet (`evidence/benchmarks-2026-09-18.json`).
- **PREVIOUSLY VERIFIED (earlier sessions, pre-hardening source):**
  `recon_crash_e2e` against a local solana-test-validator; `devnet_e2e`
  read-only against public devnet; the full `latency_bench` suite.
- **NOT EXECUTED / ENVIRONMENT-BLOCKED:** Docker image build + container
  smoke (no daemon in sandbox — CI `docker` job executes both); CI itself
  (needs GitHub runners); funded/mainnet landing-rate runs (needs funded
  keys + explicit approval); external security audit (none exists —
  `docs/SECURITY.md`).

## 4. Operating it

- Quick start, configuration precedence, env overrides, going-live gates:
  `README.md`.
- Day-two runbook (incidents, degradation matrix, emergency stop, journal,
  audit trail): `docs/OPERATIONS.md`.
- Backup/restore and Redis-loss behavior: `docs/BACKUP-RESTORE.md`.
- Multi-replica deployment (claims/leases/fencing/flag sync + honest
  limits): `docs/DISTRIBUTED.md`.
- Reconciliation/source-of-truth model: `docs/RECONCILIATION.md`.

## 5. Handover fill-ins (deliberate placeholders — act before production)

These are the only intentional open items; each is labeled in-place:

1. **LICENSE copyright holder** — replace "sniper-suite authors" with the
   legal entity transferring/receiving the rights (note at the bottom of
   `LICENSE`).
2. **Staking program id** — `programs/staking-suite/src/lib.rs`
   `declare_id!` is a pre-deploy placeholder; deploy under it with the
   matching keypair or change id+keypair and rebuild (README §"Deploying the
   staking program"). Until deployed, Module 4 cannot run live.
3. **`repository` metadata** — the workspace `Cargo.toml` intentionally has
   no `repository` URL (the previous placeholder was removed); set it to the
   real remote when published.
4. **Security contact** — root `SECURITY.md` points at "the current
   repository owner's security contact"; publish a real address.
5. **External audit of the staking program** — mandatory before mainnet
   (stated in README, `docs/SECURITY.md`, `docs/STAKING.md`).

## 6. Maintenance invariants (do not regress)

- One logical execution ⇒ at most one active owner ⇒ at most one money-moving
  submission (`docs/DISTRIBUTED.md` §1). Any new money path must go through
  risk → claim → fence → intent journal → execute → finish(ambiguous?).
- Risk checks run before execution and no module may bypass the global risk
  engine; the `GlobalRiskOracle` may only tighten limits.
- Durable financial state lives in Postgres; Redis is coordination/cache
  only and may die without losing money-relevant truth.
- The audit trail is append-only from all app APIs; hash-chain verification
  is `GET /api/audit/verify`.
- `cargo clippy --workspace --all-targets -- -D warnings` is a hard gate, as
  are fmt, audit and deny. Keep it that way in CI.
- Never weaken or delete a test to make a gate pass; skipped-by-env tests
  must announce themselves (rule in `docs/TESTING.md`).
