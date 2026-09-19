# Evidence index — claim → source map

Every major claim made anywhere in the buyer package, mapped to the file and
section that evidences it, with its verification status and date. Purpose: a
buyer's due-diligence team can check any claim in one hop. Status labels per
`docs/HANDOVER.md` §3. Dates: engineering evidence was produced 2026-09-17
(build sessions) and 2026-09-18 (release + freeze passes), as recorded in
`AUDIT.md`'s dated sections.

## Test & gate claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| 537/537 workspace tests, 0 failures (audit pass, incl. 38 gated executed; 521/521 at freeze) | `AUDIT.md`; `docs/TESTING.md`; `release-manifest.json` | §27–28; §"What is covered where"; `test_counts.workspace_total` | VERIFIED — 2026-09-18 |
| db_integration 23/23 vs real PostgreSQL 16.4 (fresh + rerun) | `AUDIT.md`; `docs/TESTING.md` | §27; db_integration bullet (line ~49) | VERIFIED — 2026-09-18 |
| redis_integration 10/10 vs real Redis 7.2.10 | `AUDIT.md`; `docs/TESTING.md` | §27; redis bullet (line ~75) | VERIFIED — 2026-09-18 |
| distributed_integration 4/4 | `AUDIT.md`; `docs/TESTING.md` | §27; distributed bullet (line ~82) | VERIFIED — 2026-09-18 |
| two_replica_mirror 1/1 (two real processes) | `AUDIT.md`; `docs/TESTING.md`; `crates/module-copy/tests/two_replica_mirror.rs` | §27; test source | VERIFIED — 2026-09-18 |
| Staking host 71/71 (audit pass; 48/48 at freeze) | `AUDIT.md`; `docs/STAKING.md`; `release-manifest.json` | §27–28; `test_counts.staking_host` | VERIFIED — 2026-09-18 |
| Staking validator e2e 2/2 on the FREEZE source (funded stake→reward→unstake, local validator) | `AUDIT.md`; `docs/STAKING.md`; `programs/staking-suite/tests/validator_e2e.rs` | earlier dated sections; e2e source | PREVIOUSLY VERIFIED — 2026-09-17 era (agave 2.1.21) — historical |
| Staking validator e2e **3/3 EXECUTED on the audit-pass source** (governance + funded lifecycle + max-supply/metadata vs REAL mainnet-cloned mpl) | `AUDIT.md` §29; `docs/STAKING.md`; hardening evidence `phase5-full-batch.log` | e2e "Testing" section | VERIFIED — 2026-09-18 (solana-test-validator 2.1.21, batch 160.72 s) |
| `cargo build-sbf` → 5,440-byte `staking_suite.so` (FREEZE source) | `AUDIT.md`; `release-manifest.json` | earlier sections | PREVIOUSLY VERIFIED — historical, superseded source |
| `cargo build-sbf` EXECUTED on the audit-pass source → 187,504-byte `staking_suite.so`, SHA-256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` | `AUDIT.md` §29; `release-manifest.json`; hardening evidence | `staking_program` | VERIFIED — 2026-09-18 (Agave 2.1.21 / platform-tools v1.43 / sbf rustc 1.79.0) |
| release-check 20 PASS / 0 FAIL / 0 SKIP, exit 0 | `AUDIT.md`; `release-manifest.json`; `scripts/release-check.sh` | §27 (definitive run on frozen tree); `test_counts.release_check_gates` | VERIFIED — 2026-09-18 |
| 609 whole-script test executions / 0 failures | `AUDIT.md` | §27 | VERIFIED — 2026-09-18 |
| fmt + clippy `-D warnings` clean (both cargo projects) | `AUDIT.md`; `release-manifest.json` | §26–27; `verified_final_pass` | VERIFIED — 2026-09-18 |
| cargo-audit ×2 = 0 findings (cargo-audit 0.22.2) | `AUDIT.md`; `release-manifest.json`; `docs/THIRD-PARTY.md` | §27; `verified_final_pass`; §5 | VERIFIED — 2026-09-18 |
| cargo-deny ok (advisories/bans/licenses/sources; 0.18.9) | `AUDIT.md`; `deny.toml`; `docs/THIRD-PARTY.md` | §27; policy file; §4 | VERIFIED — 2026-09-18 |
| recon_crash_e2e vs local solana-test-validator | `AUDIT.md`; `crates/solana-kit/tests/recon_crash_e2e.rs` | earlier dated sections | PREVIOUSLY VERIFIED |
| devnet_e2e read-only vs public devnet; latency_bench | `AUDIT.md`; `crates/solana-kit/tests/{devnet_e2e,latency_bench}.rs` | earlier dated sections | PREVIOUSLY VERIFIED |
| Deterministic ledger replay | `AUDIT.md` | earlier dated sections | PREVIOUSLY VERIFIED |

## Security & correctness claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| pg_dump → restore → full db_integration suite green on restored DB | `AUDIT.md`; `docs/BACKUP-RESTORE.md`; `docs/HANDOVER.md` | §26–27; procedures; §2 | VERIFIED — 2026-09-18 |
| Audit-chain tamper detection (modify/reorder/missing/duplicate) + linear chain under 8 concurrent appenders | `AUDIT.md`; `crates/core/tests/db_integration.rs`; `crates/core/src/db/repo.rs` | §26 (fix + tests); chain tests; advisory-lock append | VERIFIED — 2026-09-18 |
| Telegram bot-token redaction in all API error paths (+ closed-port regression test) | `AUDIT.md`; `crates/module-telegram/src/api.rs`; `CHANGELOG.md` | §27; `without_url()` sites + `error_strings_never_contain_the_bot_token`; "Fixed (engineering-freeze pass)" | VERIFIED — 2026-09-18 |
| No secrets / no build artifacts in tree; marker scan clean | `scripts/release-check.sh`; `AUDIT.md` | secret_scan + marker_scan gates; §27 | VERIFIED — 2026-09-18 |
| Migrations monotonic 0001–0011; version + toolchain-pin consistency | `scripts/release-check.sh`; `crates/core/migrations/` | migration_check + version_check + toolchain_check gates | VERIFIED — 2026-09-18 |
| RBAC: readonly-cannot-mutate, operator≠owner, live-mode owner-only | `crates/core/src/auth.rs`; `crates/module-telegram/src/commands.rs`; test suite | authz tests within the 537 | VERIFIED — 2026-09-18 |
| No external security audit exists (any component) | root `SECURITY.md`; `docs/SECURITY.md`; `release-manifest.json` | "What this document is not"; `not_executed_environment_blocked` | FACT — current |

## Packaging & metadata claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| Version 0.1.0 consistent across VERSION / Cargo.toml / manifest | `VERSION`; `Cargo.toml`; `release-manifest.json`; `scripts/release-check.sh` | version_check gate | VERIFIED — 2026-09-18 |
| Frozen tree: 146 files / 2,801,590 B / 77,980 lines; category breakdown | `docs/FINAL-DELIVERY.md`; `docs/BUYER-DUE-DILIGENCE.md` | §3; §A (measurement commands included for re-verification) | VERIFIED measurement — 2026-09-18 |
| Documentation passes changed no code bytes (byte-exact category proofs) | pass reports (`COMMERCIAL_PACKAGE_REPORT.md` §7 in the seller's workspace; re-derivable with the commands in `docs/BUYER-DUE-DILIGENCE.md` §A) | byte arithmetic: Rust 2,051,936 B, SQL 23,443 B unchanged | VERIFIED — 2026-09-18 (buyer package pass) |
| Documentation consistency clean (links, counts, versions, paths, invisible chars) | `scripts/verify-delivery.sh` (in-repo checker) | whole script | VERIFIED — re-runnable at any time |
| Docker image build + smoke not executed in delivery environment | `release-manifest.json`; `.github/workflows/ci.yml` | `not_executed_environment_blocked`; docker job | NOT EXECUTED — fact |
| CI workflow delivered, never run from delivery environment | `.github/workflows/ci.yml`; `release-manifest.json` | 4 jobs; `not_executed_environment_blocked` | NOT EXECUTED — fact |
| Funded live trading never executed | `release-manifest.json`; `docs/TESTING.md` | `not_executed_environment_blocked`; §Known gaps | NOT EXECUTED — fact |
| SBOM generator not run; lockfiles authoritative (706 + 580 packages) | `docs/THIRD-PARTY.md`; `Cargo.lock`; `programs/staking-suite/Cargo.lock` | §1, §6 | NOT EXECUTED (SBOM) / FACT (lockfiles) |
| Authoritative history `9c677cd` → `0e139c3` | `AUDIT.md`; `CHANGELOG.md`; `release-manifest.json` notes; `docs/FINAL-DELIVERY.md` | §26–27 headers; release identity; manifest design note; §2 | FACT — recorded in the authoritative repository (git metadata absent from the packaging sandbox; never fabricated) |

## Buyer-hardening pass (2026-09-18) — EXECUTED evidence

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
| 537/537 workspace tests | `cargo test --workspace -- --test-threads=1` (PG+Redis live) | `phase2-test-workspace.log` | PASS — gated suites EXECUTED |
| 537/537 with `--all-features` | same + `--all-features` | `phase2-test-workspace-allfeat.log` | PASS — no feature-gated delta |
| Staking host 71/71 + clippy `-D warnings` | `cargo test` / `cargo clippy --all-targets -- -D warnings` in `programs/staking-suite` | `phase2-staking-clippy.log` (clippy/check output only) **+ `release-check-final.log` §"staking: cargo test" (the `test result: ok. 71 passed` line)** | PASS — citation completed by the 2026-09-19 adversarial audit |
| `cargo build-sbf` artifact | `cargo build-sbf` (platform-tools v1.43) | `staking_suite.so.first` — 187,504 B, SHA-256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` | PASS — supersedes freeze-era `9e113678…` |
| build-sbf determinism | incremental (`touch src/lib.rs && cargo build-sbf`) AND full cold rebuild (reinstalled rust 1.98.1, verified agave tarball `5da3359e…`, fresh platform-tools, empty cache) | `phase3-sbf-rebuild.log` (incremental run; persisted tail TRUNCATED at the platform-tools download — kept as honest history) + `phase3-sbf-determinism-rerun.log` (COMPLETE cold re-proof, 2026-09-19) | PASS — byte-identical `57a890fa…` (cold rebuild == first build == package binary) |
| Validator e2e 3/3 (governance, funded money flow, max-supply cap + metadata) | `STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` (real solana-test-validator, Agave BPF VM, mpl clone from mainnet-beta) | `phase5-full-batch.log` (160.72 s, FINAL PASS), fix history in `phase5-capmeta-fix.log`; pre-fix FAILED attempts preserved as labeled history: `phase5-validator-e2e.log` (first 3/3 FAILED run, mpl-discriminant era), `phase5-capmeta-retry.log`, `phase5-funded-retry{,2}.log` | PASS — 2 real defects found + fixed (§29 AUDIT.md); a FAILED log in `evidence/tests/` is pre-fix history, never the final result |
| pg_dump → clean-DB restore → identity | `pg_dump -Fc` / `pg_restore --no-owner` + psql comparisons ×3 | `phase8-main.log`, `phase8-{src,rst}-{tables,migrations,rowcounts}.txt`, dump `backup-2026-09-18T15:44:22Z.dump` (SHA-256 `5989ecf1…`, `phase8-3-dump.sha256`) | PASS — 24 tables / 68 rows / 11 migrations identical |
| db_integration ON restored DB | `POSTGRES_URL=<restored> cargo test -p core --test db_integration -- --test-threads=1` | `phase8-6-restored-rerun.log` | PASS — 23/23 |
| App startup + endpoints + graceful shutdown | `./target/debug/sniper-suite` + curl battery + SIGTERM | `phase8b-{app,endpoints,shutdown,main}.log` | PASS — `/health` ok, `/ready` 200 (4 components), `/api/status` paper / live_allowed:false / kill_switch:false, `bot_*` metrics, "stopped cleanly" |
| RPC latency + simulate bench (existing tests only) | `E2E_NETWORK=1 cargo test -p solana-kit --test latency_bench -- --test-threads=1 --nocapture` | `phase11-latency-devnet.log`, machine-readable `benchmarks-2026-09-18.json` | PASS — getSlot/getLatestBlockhash/simulate legs on public devnet; landing_rate SKIP (needs funded keys — HUMAN ACTION); figures = this sandbox, not product guarantees |
| Program-ID identity verification | `./scripts/staking-identity.sh verify` | script output (all tracked refs agree; placeholder status printed) | PASS — final ID = HUMAN ACTION (buyer keypair) |
| Live-validation runbook (no auto-execution of funded steps) | procedure only | `docs/LIVE-VALIDATION.md` | WRITTEN — funded execution = HUMAN ACTION |
| Docker image build + container smoke | needs Docker daemon | none — sandbox has no daemon | **BLOCKED** — native-equivalent binary smoke PASSED (phase8b-*); `docker compose config -q` also BLOCKED |
| GitHub Actions run | needs GitHub runner | none | **BLOCKED** — local 1:1 equivalents recorded (`docs/CI-LOCAL-EQUIVALENCE.md`) |
| cargo audit ×2 / cargo deny | run inside `scripts/release-check.sh` | release-check log (0 errors; 9 allow-listed audit warnings per `.cargo/audit.toml`) | PASS |
| Source inventory (paths, bytes, lines, SHA-256 per file + tree hash) | generated scanner | `source-inventory.json`, `source-inventory.csv`, `ledger.csv` | PASS |

### Historical-record notes (independent adversarial audit, 2026-09-19)

* `verify-delivery-final.log` (hardening pass, 2026-09-18T16:45Z) reports
  `tree: 175 files, 3194670 bytes`; the final hardening tree is 175 files /
  3,196,164 bytes. The run predates the last documentation updates of that
  pass (Δ = 1,494 B of doc text; file count unchanged); its 7/7 PASS result
  is unaffected. The log is preserved unmodified as history. On the FINAL
  183-file tree, `verify-delivery.sh` was **re-executed on 2026-09-19:
  7 PASS / 0 FAIL, exit 0** (audit log in the final package under
  `evidence/handover/adversarial-audit/`).
* `release-check.sh` 20/20 was executed on the 175-file hardening tree. It
  was NOT re-run on the final 183-file tree (the audit environment has no
  Rust toolchain). Rust-level gates remain applicable (Rust sources
  byte-identical, proven per-file); doc-level gates were re-covered by the
  placeholder scan (0 defects), doccheck (CLEAN) and identity verify (PASS)
  on the final tree.

## How to re-verify anything above

1. **Whole gate:** `./scripts/release-check.sh` (needs PG + Redis) — reproduces
   every VERIFIED test/gate claim in one run.
2. **Bundle/docs:** `./scripts/verify-delivery.sh` (no toolchain needed).
3. **Individual suites:** exact commands in README §Testing and
   `docs/TESTING.md`.
4. **Sizes/counts:** commands in `docs/BUYER-DUE-DILIGENCE.md` §A.
5. **Historical narrative:** `AUDIT.md` (dated sections, oldest → newest;
   historical sections are preserved unmodified by policy).
