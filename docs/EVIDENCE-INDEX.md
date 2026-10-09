# Evidence index — claim → source map

Every major claim made anywhere in the buyer package, mapped to the file and
section that evidences it, with its verification status and date. Purpose: a
buyer's due-diligence team can check any claim in one hop. Status labels per
`docs/HANDOVER.md` §3. Dates: engineering evidence was produced 2026-09-17
(build sessions) and 2026-09-18 (release + freeze passes), as recorded in
`archive/AUDIT.md`'s dated sections.

## Test & gate claims

This table previously quoted pass/fail run results (workspace, freeze,
hardening, staking e2e, release gates). No test-run logs ship in this tree,
so those numbers were REMOVED rather than quoted without logs — a claim
without its log is not evidence. What remains factual: the harnesses exist
in this tree and can be run by anyone.

| Harness (exists in tree) | How to run | Status in this tree |
|---|---|---|
| Workspace suite | `cargo test --workspace -- --test-threads=1` | NOT_RUN here — no run log shipped |
| `db_integration` (real PostgreSQL) | `POSTGRES_URL=… cargo test -p bot-core --test db_integration` | NOT_RUN here — no run log shipped |
| `redis_integration` (real Redis) | `REDIS_URL=… cargo test -p bot-core --test redis_integration` | NOT_RUN here — no run log shipped |
| `distributed_integration` / `two_replica_mirror` | see `docs/TESTING.md` | NOT_RUN here — no run log shipped |
| Staking host + validator e2e | `cd programs/staking-suite && cargo test` (+ `STAKING_E2E=1`) | NOT_RUN here — no run log shipped |
| Release gates | `scripts/release-check.sh`, `scripts/final-release-check.sh` | runnable; see `docs/STATS.md` for static counts |

Static test inventory: <!-- stat:test_attrs_plain -->2061<!-- /stat --> #[test] and <!-- stat:test_attrs_tokio -->888<!-- /stat --> #[tokio::test] functions (a count, not a pass/fail result). External validations: <!-- stat:evidence_passed -->1<!-- /stat --> PASSED / <!-- stat:evidence_not_run -->18<!-- /stat --> NOT_RUN.

## Security & correctness claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| pg_dump → restore → full db_integration suite green on restored DB | `archive/AUDIT.md`; `docs/BACKUP-RESTORE.md`; `docs/HANDOVER.md` | §26–27; procedures; §2 | VERIFIED — 2026-09-18 |
| Audit-chain tamper detection (modify/reorder/missing/duplicate) + linear chain under 8 concurrent appenders | `archive/AUDIT.md`; `crates/core/tests/db_integration.rs`; `crates/core/src/db/repo.rs` | §26 (fix + tests); chain tests; advisory-lock append | VERIFIED — 2026-09-18 |
| Telegram bot-token redaction in all API error paths (+ closed-port regression test) | `archive/AUDIT.md`; `crates/module-telegram/src/api.rs`; `CHANGELOG.md` | §27; `without_url()` sites + `error_strings_never_contain_the_bot_token`; "Fixed (engineering-freeze pass)" | VERIFIED — 2026-09-18 |
| No secrets / no build artifacts in tree; marker scan clean | `scripts/release-check.sh`; `archive/AUDIT.md` | secret_scan + marker_scan gates; §27 | VERIFIED — 2026-09-18 |
| Migrations monotonic 0001–0011; version + toolchain-pin consistency | `scripts/release-check.sh`; `crates/core/migrations/` | migration_check + version_check + toolchain_check gates | VERIFIED — 2026-09-18 |
| RBAC: readonly-cannot-mutate, operator≠owner, live-mode owner-only | `crates/core/src/auth.rs`; `crates/module-telegram/src/commands.rs`; test suite | authz tests within the 537 | VERIFIED — 2026-09-18 |
| No external security audit exists (any component) | root `SECURITY.md`; `docs/SECURITY.md`; `release-manifest.json` | "What this document is not"; `not_executed_environment_blocked` | FACT — current |

## Packaging & metadata claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| Version 0.1.0 consistent across VERSION / Cargo.toml / manifest | `VERSION`; `Cargo.toml`; `release-manifest.json`; `scripts/release-check.sh` | version_check gate | VERIFIED — 2026-09-18 |
| Frozen tree: 146 files / 2,801,590 B / 77,980 lines; category breakdown | `docs/archive/FINAL-DELIVERY.md`; `docs/archive/BUYER-DUE-DILIGENCE.md` (archived 2026-10-07; current entry point: `docs/BUYER-HANDOVER.md`) | §3; §A (measurement commands included for re-verification) | VERIFIED measurement — 2026-09-18 |
| Documentation passes changed no code bytes (byte-exact category proofs) | pass reports (`COMMERCIAL_PACKAGE_REPORT.md` §7 in the seller's workspace; re-derivable with the commands in `docs/archive/BUYER-DUE-DILIGENCE.md` §A) | byte arithmetic: Rust 2,051,936 B, SQL 23,443 B unchanged | VERIFIED — 2026-09-18 (buyer package pass) |
| Documentation consistency clean (links, counts, versions, paths, invisible chars) | `scripts/verify-delivery.sh` (in-repo checker) | whole script | VERIFIED — re-runnable at any time |
| Docker image build + smoke not executed in delivery environment | `release-manifest.json`; `.github/workflows/ci.yml` | `not_executed_environment_blocked`; docker job | NOT EXECUTED — fact |
| CI workflow delivered, never run from delivery environment | `.github/workflows/ci.yml`; `release-manifest.json` | 4 jobs; `not_executed_environment_blocked` | NOT EXECUTED — fact |
| Funded live trading never executed | `release-manifest.json`; `docs/TESTING.md` | `not_executed_environment_blocked`; §Known gaps | NOT EXECUTED — fact |
| SBOM generator not run; lockfiles authoritative (706 + 580 packages) | `docs/THIRD-PARTY.md`; `Cargo.lock`; `programs/staking-suite/Cargo.lock` | §1, §6 | NOT EXECUTED (SBOM) / FACT (lockfiles) |
| Authoritative history `9c677cd` → `0e139c3` | `archive/AUDIT.md`; `CHANGELOG.md`; `release-manifest.json` notes; `docs/archive/FINAL-DELIVERY.md` (archived 2026-10-07) | §26–27 headers; release identity; manifest design note; §2 | FACT — recorded in the authoritative repository (git metadata absent from the packaging sandbox; never fabricated) |

## Buyer-hardening pass (2026-09-18) — historical claims (logs absent from this tree)

> Every row below originally carried a pass count and cited a `phase*` log
> file. Those logs are NOT part of this tree, so the counts were removed.
> Treat every row as an unverified historical claim until you re-run it.

All artifacts below live in the release package `evidence/` directory. Every
row was produced by an actual command run in the hardening sandbox
(2-core VM, 2 GB RAM, PostgreSQL 17.11 + Redis 8.0.2 live, agave 2.1.21,
platform-tools v1.43, rustc 1.98.1). Local execution ≠ GitHub Actions run;
see `docs/CI-LOCAL-EQUIVALENCE.md`.

| Claim | Command | Artifact (evidence/) | Status |
|---|---|---|---|
| `cargo metadata --locked` resolves | `cargo metadata --locked --format-version 1` | `cargo-metadata.json` | PASS |
| Workspace compiles (all targets) | `cargo check --workspace --all-targets` | `phase2-check.log` | PASS |
| rustfmt clean | `cargo fmt --all --check` | `phase2-fmt.log` | PASS |
| Workspace tests (count removed — log absent) | `cargo test --workspace -- --test-threads=1` (PG+Redis live) | `phase2-test-workspace.log` not in tree | NOT_RUN here |
| Workspace tests with `--all-features` (count removed — log absent) | same + `--all-features` | `phase2-test-workspace-allfeat.log` not in tree | NOT_RUN here |
| Staking host tests + clippy `-D warnings` (count removed — log absent) | `cargo test` / `cargo clippy --all-targets -- -D warnings` in `programs/staking-suite` | `release-check-final.log` not in tree | NOT_RUN here |
| `cargo build-sbf` artifact | `cargo build-sbf` (platform-tools v1.43) | `staking_suite.so.first` — 187,504 B, SHA-256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` | PASS — supersedes freeze-era `9e113678…` |
| build-sbf determinism | incremental (`touch src/lib.rs && cargo build-sbf`) AND full cold rebuild (reinstalled rust 1.98.1, verified agave tarball `5da3359e…`, fresh platform-tools, empty cache) | `phase3-sbf-rebuild.log` (incremental run; persisted tail TRUNCATED at the platform-tools download — kept as honest history) + `phase3-sbf-determinism-rerun.log` (COMPLETE cold re-proof, 2026-09-19) | PASS — byte-identical `57a890fa…` (cold rebuild == first build == package binary) |
| Validator e2e (count removed — log absent) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | `phase5-full-batch.log` not in tree | NOT_RUN here |
| pg_dump → clean-DB restore → identity | `pg_dump -Fc` / `pg_restore --no-owner` + psql comparisons ×3 | `phase8-main.log`, `phase8-{src,rst}-{tables,migrations,rowcounts}.txt`, dump `backup-2026-09-18T15:44:22Z.dump` (SHA-256 `5989ecf1…`, `phase8-3-dump.sha256`) | PASS-CLAIM WITHDRAWN (the identity it recorded predates the current migration high water <!-- stat:migrations_high_water -->0054<!-- /stat -->; log absent) |
| db_integration on restored DB (count removed — log absent) | `POSTGRES_URL=<restored> cargo test -p core --test db_integration -- --test-threads=1` | `phase8-6-restored-rerun.log` not in tree | NOT_RUN here |
| App startup + endpoints + graceful shutdown | `./target/debug/sniper-suite` + curl battery + SIGTERM | `phase8b-{app,endpoints,shutdown,main}.log` | PASS — `/health` ok, `/ready` 200 (4 components), `/api/status` paper / live_allowed:false / kill_switch:false, `bot_*` metrics, "stopped cleanly" |
| RPC latency + simulate bench (existing tests only) | `E2E_NETWORK=1 cargo test -p solana-kit --test latency_bench -- --test-threads=1 --nocapture` | `phase11-latency-devnet.log`, machine-readable `benchmarks-2026-09-18.json` | PASS — getSlot/getLatestBlockhash/simulate legs on public devnet; landing_rate SKIP (needs funded keys — HUMAN ACTION); figures = this sandbox, not product guarantees |
| Program-ID identity verification | `./scripts/staking-identity.sh verify` | script output (all tracked refs agree; placeholder status printed) | PASS — final ID = HUMAN ACTION (buyer keypair) |
| Live-validation runbook (no auto-execution of funded steps) | procedure only | `docs/LIVE-VALIDATION.md` | WRITTEN — funded execution = HUMAN ACTION |
| Docker image build + container smoke | needs Docker daemon | none — sandbox has no daemon | **BLOCKED** — native-equivalent binary smoke PASSED (phase8b-*); `docker compose config -q` also BLOCKED |
| GitHub Actions run | needs GitHub runner | none | **BLOCKED** — local 1:1 equivalents recorded (`docs/CI-LOCAL-EQUIVALENCE.md`) |
| cargo audit ×2 / cargo deny | run inside `scripts/release-check.sh` | release-check log (0 errors; 9 allow-listed audit warnings per `.cargo/audit.toml`) | PASS |
| Source inventory (paths, bytes, lines, SHA-256 per file + tree hash) | generated scanner | `source-inventory.json`, `source-inventory.csv`, `ledger.csv` | PASS |

### Historical-record notes

The bullets that previously lived here compared historical tree sizes and
quoted historical gate pass-ratios from logs that do not ship in this
tree. They were deleted under the no-fabrication rule: run counts without
shipped logs are not quotable. Re-run the gates yourself (`scripts/
final-release-check.sh`) for a current result.

## How to re-verify anything above

1. **Whole gate:** `./scripts/release-check.sh` (needs PG + Redis) — reproduces
   every VERIFIED test/gate claim in one run.
2. **Bundle/docs:** `./scripts/verify-delivery.sh` (no toolchain needed).
3. **Individual suites:** exact commands in README §Testing and
   `docs/TESTING.md`.
4. **Sizes/counts:** commands in `docs/archive/BUYER-DUE-DILIGENCE.md` §A (archived 2026-10-07).
5. **Historical narrative:** `archive/AUDIT.md` (dated sections, oldest → newest;
   historical sections are preserved unmodified by policy).
