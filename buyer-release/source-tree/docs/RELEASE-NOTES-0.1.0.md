# Release notes — sniper-suite 0.1.0 (buyer edition)

Factual release summary for the receiving party. The full engineering history
is in `CHANGELOG.md`; the evidence trail is in `AUDIT.md`; this document does
not duplicate either — it states what the release *is*, what was proven, and
what remains open.

## Release identity

| Field | Value |
|---|---|
| Product | sniper-suite — modular crypto trading system (5 modules + control plane) |
| Version | `0.1.0` (initial handover release; `VERSION`, root `Cargo.toml`, `release-manifest.json` agree — gated by `scripts/release-check.sh`) |
| License | MIT (`LICENSE`; copyright holder is a documented handover placeholder) |
| Release commit | `9c677cd` |
| Engineering-freeze commit | `0e139c3` (the frozen tree this package describes) |
| Tree at freeze | 146 tracked files, 2,801,590 bytes (2.80 MB), 77,980 lines |
| Toolchain | Rust 1.98.1 pinned (`rust-toolchain.toml`, Dockerfile, CI); MSRV 1.82 (app) / 1.79 (staking program); agave 2.1.21 for `build-sbf` |

## Test results (final freeze gate, executed on commit `0e139c3`)

| Suite | Result |
|---|---|
| `scripts/release-check.sh` | **20 PASS / 0 FAIL / 0 SKIP**, exit 0 |
| Workspace tests (`--test-threads=1`) | **521 / 521** passed (incl. 38 gated integration tests executed against real services) |
| `db_integration` (PostgreSQL 16.4) | 23 / 23 (fresh + rerun + pg_dump→restore→suite-green round-trip) |
| `redis_integration` (Redis 7.2.10) | 10 / 10 |
| `distributed_integration` | 4 / 4 |
| `two_replica_mirror` (two real processes) | 1 / 1 |
| Staking host tests | 48 / 48 (+2 validator e2e gated-skipped in the freeze sandbox) |
| fmt / `clippy -D warnings` (both projects) | clean |
| `cargo audit` (both lockfiles) | 0 findings |
| `cargo deny` (advisories/bans/licenses/sources) | ok |
| Total test executions across the gate script | 609, 0 failures |

Machine-readable copy: `release-manifest.json` → `test_counts`.

## Major components delivered

- **Modules:** sniper (pump.fun launch detection + PumpSwap/Raydium/Jupiter
  exits), copy trading, Polymarket (Gamma/CLOB, EIP-712 v2 signing), native
  Solana staking program (timelock, caps, two-step admin, latched genesis
  mint), Telegram control (deny-by-default RBAC).
- **Control plane:** Axum REST (23 endpoints over 21 `/api` routes) +
  WebSocket feed + 4 infra routes (28 documented in `docs/API.md`), embedded
  dashboard, liveness/readiness probes, bounded-label Prometheus metrics,
  request-ID correlation, rate limits, non-loopback-bind refusal without auth.
- **Core guarantees:** global pre-trade risk engine, OMS with idempotency,
  three-level restart-safe dedup, intent journal + startup reconciliation,
  hash-chained append-only audit trail, distributed execution ownership
  (claims/leases/epochs/fencing, tighten-only `GlobalRiskOracle`), PostgreSQL
  as durable truth with 11 forward-only migrations, Redis strictly
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
  secret leakage paths, docs matched to source (details: `AUDIT.md` §26–27).

## Known limitations & external blockers at release

- **No external security audit** of any component; the staking program's
  mainnet deployment is documentation-blocked until one passes.
- Staking `declare_id!` is a **pre-deploy placeholder**; the program is not
  deployed to any cluster.
- **NOT EXECUTED (environment-blocked, wired into CI or requiring external
  resources):** Docker image build + container smoke (no daemon in the build
  sandbox), GitHub Actions CI run (no runner), funded live-trading /
  mainnet landing-rate validation.
- **Executed in the buyer-hardening pass (2026-09-18, this tree — after the
  freeze numbers above):** `build-sbf` (187,504-byte .so, SHA-256
  `57a890fa…`, byte-identical rebuild), all 3 validator e2e (160.72 s),
  workspace 537/537 (incl. `--all-features`), db backup→restore round-trip
  (PostgreSQL 17.11) + app startup against the restored DB, `latency_bench`
  read-only + simulate legs, SBOM evidence (cargo metadata/tree + lockfile
  hashes). See CHANGELOG [Unreleased] + `AUDIT.md` §29.
- Legal/identity fill-ins: LICENSE copyright holder, repository URL, security
  contact (`docs/HANDOVER.md` §5, `docs/ACCEPTANCE-CHECKLIST.md`).

## What is explicitly NOT claimed

No market-value or sale-price claim; no "guaranteed 1-second execution"
claim (the ~1s sniper figure is a design target measured only by the
PREVIOUSLY VERIFIED local `latency_bench`); no live-trading profit claim
(funded live operation was never executed); no external-audit claim; no
customer/reference claim.
