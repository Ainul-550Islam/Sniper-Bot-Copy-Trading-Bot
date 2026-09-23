# Buyer due-diligence checklist

How a technical buyer can independently verify every material claim about
sniper-suite 0.1.0. Each item states what to check, where the evidence lives,
and what the last known result was. Nothing here requires trusting this
document: every check is reproducible from the source tree.

## A. Source

| Check | How | Last known result |
|---|---|---|
| Repository structure | `git ls-files` / directory listing: `crates/` (7 workspace crates), `programs/staking-suite` (standalone), `docs/`, `scripts/`, `.github/workflows/`, root metadata files | Matches layout in README §"Project layout" |
| File count | `git ls-files \| wc -l` | 146 tracked files at freeze commit `0e139c3` (160 after this buyer-package documentation pass adds 14 docs; no source files changed) |
| Source size | `git ls-files -z \| xargs -0 wc -c` (bytes) and `wc -l` (lines) | 2,801,590 bytes (2.80 MB), 77,980 lines at freeze; by category: production Rust (src/, incl. inline unit tests) 76 files / 1,826,288 B / 50,164 lines; test-dir Rust 15 / 225,648 / 6,476; SQL migrations 11 / 23,443 / 489; docs (Markdown) 17 / 293,952 / 4,265; config + deploy + lockfiles + scripts + manifest 27 / 432,259 / 16,586 |
| Language | All application code is Rust (91 `.rs` files); SQL migrations (11); shell (release-check); TOML/YAML config | No other production languages |
| Crates | Workspace members: `bot-core`, `solana-kit`, `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `sniper-suite` (server binary) | Root `Cargo.toml` `[workspace] members` |
| Standalone staking program | `programs/staking-suite` — own `Cargo.lock`, excluded from workspace, `crate-type = ["cdylib","lib"]`, release profile `overflow-checks=true, lto="fat", panic="abort"` | Native Solana program, no Anchor |
| No build artifacts / secrets in tree | `git status`, `.gitignore`, `.dockerignore`; release-check secret-scan + marker-scan gates | Clean; scans pass (`scripts/release-check.sh`) |

## B. Build

| Check | How | Last known result |
|---|---|---|
| Toolchain pin | `rust-toolchain.toml` (1.98.1 + rustfmt + clippy); Dockerfile base `rust:1.98.1-bookworm`; CI `dtolnay/rust-toolchain@1.98.1`; MSRV `rust-version = 1.82` (app) / `1.79` (program, for agave platform-tools resolver) | Three-way pin consistency is a release-check gate |
| Lockfiles | `Cargo.lock` (706 packages) and `programs/staking-suite/Cargo.lock` (580 packages), both committed | `cargo build --locked` reproducible dependency graph |
| Build from zero | Fresh machine: rustup → `cargo build --release`; full procedure in `docs/HANDOVER.md` §2 | VERIFIED — whole suite rebuilt and re-gated from source alone on a wiped machine (fresh rustup, PG 16.4 from official tarball, Redis 7.2.10 from source, empty target dir, empty database) |
| Reproducibility caveats | `docs/RELEASE.md` §reproducible-build analysis: no build timestamps embedded; `release-manifest.json` deliberately omits timestamps and its own commit hash | Bit-for-bit binary reproducibility NOT claimed (rustc/OS toolchain variance); source-level reproducibility is what is verified |
| On-chain build | `cd programs/staking-suite && cargo build-sbf` | EXECUTED on the audit-pass source (hardening pass 2026-09-18, agave 2.1.21 / platform-tools v1.43): 187,504-byte `staking_suite.so`, SHA-256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`; all 3 validator e2e EXECUTED and PASSED against this exact binary. (The freeze-era 5,440-byte figure refers to the superseded freeze source.) |

## C. Testing

| Check | How | Last known result |
|---|---|---|
| Exact latest counts | `release-manifest.json` `test_counts`; `docs/TESTING.md`; `CHANGELOG.md` | Latest source pass (2026-09-22): workspace 1028 passed / 0 failed / 1 intentionally ignored; PostgreSQL 17.11 live; db_integration 26/26; both SaaS durability tests executed; workspace check and strict Clippy passed. Historical hardening pass: 537/537 with PostgreSQL + Redis; freeze gate: 521/521 + staking 48/48. |
| Reproduce yourself | `./scripts/release-check.sh` with `POSTGRES_URL` + `REDIS_URL` exported (procedure: `docs/HANDOVER.md` §2) | One command re-runs the entire gate |
| Historical external verification | PREVIOUSLY VERIFIED on identical source: `build-sbf`, validator e2e 2/2 (incl. funded stake→reward→unstake on local validator), `recon_crash_e2e`, devnet `devnet_e2e` read-only, `latency_bench`, deterministic ledger replay | Listed with labels in `docs/HANDOVER.md` §3 and `release-manifest.json` `verification_status` |
| Gated checks | Env-gated suites announce themselves and skip cleanly (`POSTGRES_URL`, `REDIS_URL`, `STAKING_E2E`, `E2E_NETWORK`, `E2E_LIVE`); rule: a skipped test must never look like a pass | `docs/TESTING.md` §Rules |
| Never-executed items | Docker build+smoke (no daemon), GitHub CI run (no runner), funded live trading, external audit | Explicitly NOT EXECUTED in `release-manifest.json`; also `docs/BUYER-RISK-REGISTER.md` |

## D. Security

| Check | How | Last known result |
|---|---|---|
| Authentication | API key (`x-api-key`) on mutating routes; server refuses non-loopback bind without auth; Telegram deny-by-default allow-lists | `docs/API.md`, `docs/SECURITY.md`; authz tests part of the 521 |
| Authorization / RBAC | Roles: API key holders; Telegram `owner` / `operator` / `readonly`; `/mode live` and key/journal mutations owner-only; readonly cannot mutate; operator ≠ owner | `crates/core/src/auth.rs`, `crates/module-telegram/src/commands.rs`; regression tests in suite |
| Signer boundary | Modules never touch key material; `TransactionSigner` + `SignerRegistry`; undeclared/unresolvable extra signer = structured build failure; `vault`/`kms`/`hsm` providers fail startup | `crates/solana-kit/src/signer.rs`; `docs/SECURITY.md` |
| Secret handling | Env-var indirection (`*_env` fields), no secrets in repo (release-check secret scan), redacted `/api/config`, bounded metric labels exclude secrets, Telegram token stripped from all error strings (`Error::without_url()` × 10 sites + regression test) | Freeze-pass defect found & fixed; see CHANGELOG "Fixed (engineering-freeze pass)" |
| Audit chain | Append-only from all app APIs; hash-chained; `GET /api/audit/verify`; advisory-lock serialized appends | Tamper detection + 8-appender concurrency tests VERIFIED vs real PG |
| External audit status | **None exists.** No external security audit, penetration test, or formal verification of any component. Staking program mainnet deployment is documentation-blocked until an independent audit passes | Root `SECURITY.md`, `docs/SECURITY.md`, `docs/STAKING.md` |

## E. Infrastructure

| Check | How | Last known result |
|---|---|---|
| PostgreSQL | Required ≥ 16 (verified on 16.4); 11 forward-only migrations embedded via sqlx; `auto_migrate` at startup | db_integration 23/23 + pg_dump→restore round-trip VERIFIED |
| Redis | Required 7.x (verified on 7.2.10); non-authoritative only | redis_integration 10/10 VERIFIED; Redis-loss behavior in `docs/BACKUP-RESTORE.md` |
| RPC / WebSocket | Solana JSON-RPC + WS configured via `RPC_URL`/`WS_URL`; retry/failover/fan-out in `crates/solana-kit/src/rpc.rs` | Mock-verified; production provider is buyer-supplied |
| Geyser | Yellowstone-compatible `transactionSubscribe` endpoint via `GEYSER_WS_URL`; poll fallback when absent | Mock-verified (`geyser_detect`, `geyser_feed`); real provider NOT EXECUTED |
| Docker | `Dockerfile` (multi-stage, non-root, healthcheck), `docker-compose.yml` (bot + postgres:16-alpine + redis:7-alpine, healthcheck-gated, loopback API publish) | Static inspection + CI `docker compose config` gate only; image build NOT EXECUTED (no daemon in sandbox) |
| CI | `.github/workflows/ci.yml`: app workspace (fmt/clippy `-D warnings`/build/test with PG16+Redis7 service containers), staking program (host tests + build-sbf + gated validator e2e), security (audit ×2 + deny incl. licenses), docker (build + container health smoke) | Workflow file delivered; a CI *run* requires the buyer's GitHub — NOT EXECUTED here; equivalent steps VERIFIED locally via release-check |

## F. Operations

| Check | How | Last known result |
|---|---|---|
| Startup / shutdown | Ordered module supervision, readiness gating, graceful shutdown | `docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`; lifecycle tests in suite |
| Recovery | Intent journal replay + reconciliation before new work; handoff grace for ambiguous outcomes | `docs/RECONCILIATION.md`; restart-recovery tests VERIFIED; `recon_crash_e2e` PREVIOUSLY VERIFIED |
| Backup / restore | pg_dump/restore procedure + journal file handling; Redis is disposable | pg_dump→restore→full-suite-green VERIFIED; `docs/BACKUP-RESTORE.md` |
| Monitoring | `/health` (liveness), `/ready` (readiness), `/metrics` (Prometheus), request-ID correlated JSON logs | `docs/OPERATIONS.md` runbook; metric names verified to match source (11/11) |

## G. Ownership / IP

| Check | How | Last known result |
|---|---|---|
| License | MIT (`LICENSE`); cargo-deny license allow-list gates all dependencies (`deny.toml`) | Deny licenses gate VERIFIED clean |
| Copyright holder | `LICENSE` currently reads "sniper-suite authors" — **deliberate placeholder**; the legal entity must be inserted at transfer | BUYER/SELLER ACTION (`docs/HANDOVER.md` §5.1) |
| Repository URL | Workspace `Cargo.toml` intentionally has **no** `repository` URL (placeholder was removed rather than faked) | BUYER ACTION on publish |
| Security contact | Root `SECURITY.md` points at "the current repository owner's security contact" — no real address published | BUYER/SELLER ACTION |
| Staking program identity | `declare_id!("3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy")` is a **pre-deploy placeholder**; program not deployed to any cluster | BUYER ACTION (deploy under it or change id+keypair and rebuild) |
| Component provenance | Original application code vs third-party protocol integration, itemized | `docs/IP-COMPONENTS.md`, `docs/THIRD-PARTY.md` |

## H. Open external actions (complete list)

These are the only open items at handover. None is a software defect; all are
external/human actions (mirrored in `release-manifest.json`
`external_handover_blockers` and `docs/ACCEPTANCE-CHECKLIST.md`):

1. Insert the legal copyright holder into `LICENSE`.
2. Publish a real security contact (root `SECURITY.md`).
3. Set the real repository URL when published.
4. Deploy the staking program and finalize its program id.
5. Commission an independent external security audit before any mainnet
   deployment of the staking program.
6. Provide production infrastructure: PostgreSQL ≥ 16, Redis 7, RPC/WS
   (and optionally Geyser) providers, funded keys, secret store.
7. Execute Docker image build + CI on real runners.
8. Funded live-trading validation under operator supervision (paper mode is
   the default and nothing broadcasts without both live gates).
