# Release notes — sniper-suite 0.1.0 (buyer edition)

Factual release summary for the receiving party. The full engineering history
is in `CHANGELOG.md`; the evidence trail is in `archive/AUDIT.md`; this document does
not duplicate either — it states what the release *is*, what was proven, and
what remains open.

## Release identity

| Field | Value |
|---|---|
| Product | sniper-suite — modular crypto trading system (5 modules + control plane) |
| Version | `0.1.0` (initial handover release; `VERSION`, root `Cargo.toml`, `release-manifest.json` agree — gated by `scripts/release-check.sh`) |
| License | Proprietary (`LICENSE`; all rights reserved — earlier MIT wording withdrawn) |
| Release commit | `9c677cd` |
| Engineering-freeze commit | `0e139c3` (the frozen tree this package describes) |
| Tree at freeze | 146 tracked files, 2,801,590 bytes (2.80 MB), 77,980 lines |
| Toolchain | Rust 1.98.1 pinned (`rust-toolchain.toml`, Dockerfile, CI); MSRV 1.82 (app) / 1.79 (staking program); agave 2.1.21 for `build-sbf` |

## Test suites (harnesses present in this tree)

The table that used to live here quoted per-suite pass counts from the
freeze gate. Those run logs do not ship in this tree, so the numbers were
removed. The harnesses exist and anyone can re-run them (`docs/TESTING.md`).

| Suite (harness exists) | How to run |
|---|---|
| Release gates | `scripts/release-check.sh` |
| Workspace tests | `cargo test --workspace -- --test-threads=1` |
| `db_integration` | `POSTGRES_URL=… cargo test -p bot-core --test db_integration` |
| `redis_integration` | `REDIS_URL=… cargo test -p bot-core --test redis_integration` |
| `distributed_integration` / `two_replica_mirror` | `docs/TESTING.md` |
| Staking host + validator e2e | `cd programs/staking-suite && cargo test` (+ `STAKING_E2E=1`) |
| fmt / `clippy -D warnings` / `cargo audit` | standard toolchain commands |

Static test inventory: <!-- stat:test_attrs_plain -->2061<!-- /stat --> #[test] and <!-- stat:test_attrs_tokio -->888<!-- /stat --> #[tokio::test] functions (a count, not a pass/fail result). External validations: <!-- stat:evidence_passed -->1<!-- /stat --> PASSED / <!-- stat:evidence_not_run -->18<!-- /stat --> NOT_RUN.
| `cargo deny` (advisories/bans/licenses/sources) | ok |
| Total test executions across the gate script | 609, 0 failures |

Machine-readable copy: `release-manifest.json` → `test_counts`.

## Major components delivered

- **Modules:** sniper (pump.fun launch detection + PumpSwap/Raydium/Jupiter
  exits), copy trading, Polymarket (Gamma/CLOB, EIP-712 v2 signing), native
  Solana staking program (timelock, caps, two-step admin, latched genesis
  mint), Telegram control (deny-by-default RBAC).
- **Control plane:** Axum REST (documented endpoints over `/api` routes; see `docs/API.md` and the OpenAPI spec) +
  WebSocket feed + 4 infra routes (28 documented in `docs/API.md`), embedded
  dashboard, liveness/readiness probes, bounded-label Prometheus metrics,
  request-ID correlation, rate limits, non-loopback-bind refusal without auth.
- **Core guarantees:** global pre-trade risk engine, OMS with idempotency,
  three-level restart-safe dedup, intent journal + startup reconciliation,
  hash-chained append-only audit trail, distributed execution ownership
  (claims/leases/epochs/fencing, tighten-only `GlobalRiskOracle`), PostgreSQL
  as durable truth with <!-- stat:migrations -->54<!-- /stat --> forward-only migrations, Redis strictly
  non-authoritative.
- **Packaging:** Dockerfile (multi-stage, non-root, healthchecked) +
  compose stack, CI workflow (4 jobs), `deny.toml`, `release-check.sh`,
  `release-manifest.json`, 13 engineering docs + 14 buyer-package docs.

## Major security fixes (made before the release cut)

1. **Telegram bot-token leak into error strings** (found and fixed in the
   engineering-freeze pass): the Bot API embeds the token in request URLs and
   `reqwest::Error`'s `Display` appends ` for url (…)`, so failed Telegram
   calls could put the token into logs/audit/alert text. All 10 error-mapping
   sites now strip the URL (`Error::without_url()`); regression test
   `error_strings_never_contain_the_bot_token` fails if the token ever
   reappears in an error string.
2. **Audit-chain append serialization** (release-engineering pass): concurrent
   appends could fork the hash chain under READ COMMITTED; appends are now
   serialized by a transaction-scoped advisory lock, with regression tests
   for concurrent linearization and reordered/missing/duplicate detection.

## Major engineering fixes (made before the release cut)

- Toolchain-pin drift removed (Dockerfile `rust:1.82` → `rust:1.98.1-bookworm`,
  CI program job `stable` → pinned 1.98.1); three-way pin consistency is now a
  release gate.
- Unused direct dependencies removed after proof of zero code references
  (`tokio-util`, `sha3`, `serde_with`); full gate re-run afterwards.
- Placeholder `repository` URL removed rather than faked; stale test counts
  and route-count documentation corrected; `release-manifest.json` introduced
  and wired into the gate.
- Full source-freeze audit (21 items) passed: no debug hacks, no dead code
  beyond one justified `allow(dead_code)`, no production `unwrap()`, no
  secret leakage paths, docs matched to source (details: `archive/AUDIT.md` §26–27).

## Known limitations & external blockers at release

- **No external security audit** of any component; the staking program's
  mainnet deployment is documentation-blocked until one passes.
- Staking `declare_id!` is a **pre-deploy placeholder**; the program is not
  deployed to any cluster.
- **NOT EXECUTED (environment-blocked, wired into CI or requiring external
  resources):** Docker image build + container smoke (no daemon in the build
  sandbox), GitHub Actions CI run (no runner), funded live-trading /
  mainnet landing-rate validation.
- **Claimed in the buyer-hardening pass (2026-09-18) — logs absent:** the
  bullet that used to sit here quoted a `build-sbf` digest, validator-e2e
  timing, workspace pass counts and a backup→restore round-trip. Those run
  logs do not ship in this tree, so the figures were removed. Re-run per
  `docs/TESTING.md`; the seller's own record is `archive/AUDIT.md` §29.
- Legal/identity fill-ins: LICENSE copyright holder, repository URL, security
  contact (`docs/HANDOVER.md` §5, `docs/ACCEPTANCE-CHECKLIST.md`).

## What is explicitly NOT claimed

No market-value or sale-price claim; no "guaranteed 1-second execution"
claim (the ~1s sniper figure is a design target measured only by the
PREVIOUSLY VERIFIED local `latency_bench`); no live-trading profit claim
(funded live operation was never executed); no external-audit claim; no
customer/reference claim.
