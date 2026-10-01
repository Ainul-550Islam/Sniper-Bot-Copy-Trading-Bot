# FINAL-SOURCE-VALIDATION

Exact validation record for the FINAL-COMPLETE-SOURCE release. Every row
below is a command that was actually executed on 2026-10-01 against the
staged source tree / the working repository it was copied from. No
environment failure is recorded as PASS; resource failures are recorded as
what they are and the successful retry is a separate row.

Classification vocabulary: PASS / FAIL / RESOURCE FAILURE (environment) /
NOT RUN (with reason) — never a silent conversion.

## Environment

- Sandbox: 25 GB disk quota, 1.98 GB RAM + 4 GB swap (swap enabled before any
  linking step; without it the final link of large binaries is OOM-killed on
  this machine class — previously verified in the 2026-09-30 validation).
- Rust: 1.98.1 via `rust-toolchain.toml` pin (rustup, minimal profile,
  rustfmt + clippy components), installed with the repository's own
  `rustup-init.sh`.
- Services: PostgreSQL 17.11 (role `sniper`, db `sniper` — mirrors the CI
  service container) and Redis 7 (`redis://localhost:6379`).
- Node v20.20.2 / npm 10.8.2 (same major as the CI `setup-node` pin of 20).
- Security tools: cargo-audit 0.22.2 and cargo-deny 0.20.2 (prebuilt
  upstream binaries; advisory DB fetched 2026-10-01).
- One environment adaptation: the first full build ran the default dev
  profile and EXHAUSTED the 25 GB disk quota (the `target/` tree reached
  14 GB before the remaining ~140 test binaries were linked; the linker
  failed with ENOSPC surfaced as `error: linking with cc failed: exit
  status: 1` on the `billing_authoritative_state` test binary, and the
  follow-on program-workspace downloads failed with `Error code 13:
  database or disk is full`). The successful rerun uses
  `CARGO_PROFILE_DEV_DEBUG=0` (same source, same tests, standard practice
  for constrained environments). GitHub CI runs the default profile on
  larger runners; this adaptation changes no source and no test semantics.
- One environment artifact: the turn-start sandbox restore drops executable
  bits on shell scripts. Six release gates initially reported FAIL because
  their gate scripts lost `+x`; after `chmod +x scripts/*.sh
  tests/**/*.sh` every one of them PASSes. This is an environment artifact,
  not a repository defect — the ZIP carries correct executable modes.

## Rust — app workspace (.github/workflows/ci.yml `app` job recipe)

| # | Command | Result | Detail |
|---|---|---|---|
| V1 | `cargo fmt --all --check` | PASS (exit 0, 1 s) | formatting clean |
| V2 | `cargo clippy --workspace --all-targets -- -D warnings` (`-j 1`) | PASS (exit 0, 6 m 46 s) | zero warnings under `-D warnings` |
| V8 (attempt 1) | `cargo build --workspace --all-targets` (default dev profile, `-j 1`) | RESOURCE FAILURE (exit 101, 669 s) | disk quota exhausted (25 GB at 100%; `target/` = 14 GB). Linker ENOSPC on the `billing_authoritative_state` test binary. Classified: resource failure, not a source failure — the same code links and tests green below. |
| V8b | `cargo build --workspace --all-targets` (`-j 1`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0`) | PASS (exit 0, 12 m 23 s) | all targets of all 8 workspace crates compiled and linked |
| V9 (attempt 1) | `cargo test --workspace -- --test-threads=1` | RESOURCE FAILURE (exit 101, 2 s) | aborted immediately: build artifacts unavailable after V8's disk failure |
| V9b | `POSTGRES_URL=postgres://sniper:sniper@localhost:5432/sniper REDIS_URL=redis://localhost:6379 cargo test --workspace -- --test-threads=1` (`CARGO_PROFILE_DEV_DEBUG=0`) | **PASS (exit 0, 562 s)** | **107 test binaries; 3,317 passed; 0 failed; 14 ignored** (live-gated by design). Includes 8 doc-test sections. The PostgreSQL/Redis-gated integration binaries genuinely executed: `db_integration`, `redis_integration`, `billing_integration`, `postgres_saas_integration`, `tenant_lifecycle_integration`. |

The 14 ignored tests are the live-provider opt-in gates, ignored by design
exactly as in CI's non-secret jobs: `paddle_live_checkout_ignored_without_live_billing`,
`stripe_live_checkout_ignored_without_live_billing`,
`live_stripe_requires_real_keys` (each hit by two suites),
`live_billing_requires_explicit_opt_in`,
`live_billing_never_hardcoded_success`,
`live_billing_uses_real_provider_not_fake`,
`live_custody_requires_explicit_opt_in`,
`live_custody_no_local_fallback`,
`live_custody_never_extracts_private_key`,
`regenerate_replay_fixtures` (explicit-run fixture generator), and one
doc-test (`db::mod_tenant_exports`). They require `LIVE_BILLING=1` /
`LIVE_CUSTODY=1` credentials and are recorded NOT RUN (live-gated) — see
docs/FINAL-BUYER-GAP-LEDGER.md.

## Rust — staking program workspace (ci.yml `program` job recipe, host part)

| # | Command | Result | Detail |
|---|---|---|---|
| V10 | `cargo fmt --check` (in `programs/staking-suite`) | PASS (exit 0) | |
| V11 (attempt 1) | `cargo clippy --all-targets -- -D warnings` | RESOURCE FAILURE (exit 101, 1 s) | crate download interrupted by the disk-full condition |
| V11b | `cargo clippy --all-targets -- -D warnings` | PASS (exit 0, 1 m 38 s) | zero warnings |
| V12 (attempt 1) | `cargo test` | RESOURCE FAILURE (exit 101, 0 s) | same disk-full condition |
| V12b | `cargo test` | PASS (exit 0, 92 s) | **74 passed; 0 failed** (71 unit + 3 integration; validator_e2e correctly requires `STAKING_E2E=1`) |
| — | `cargo build-sbf` | NOT RUN | requires Solana/Agave 2.1.21 + platform-tools (not installable within this sandbox's disk quota); ci.yml runs it on GitHub runners. The verified build provenance is recorded in `release-manifest.json` (`staking_program` block). |
| — | `STAKING_E2E=1 cargo test --test validator_e2e` | NOT RUN | requires `solana-test-validator` + compiled `.so` (GAP-005); CI-gated. |

## Security (ci.yml `security` job recipe)

| # | Command | Result | Detail |
|---|---|---|---|
| V13 | `cargo audit --file Cargo.lock` (cargo-audit 0.22.2) | PASS (exit 0) | 10 warnings, all allowlisted in `.cargo/audit.toml`; no unallowed vulnerabilities |
| V14 | `cargo audit --file programs/staking-suite/Cargo.lock` | PASS (exit 0) | 9 warnings, all allowlisted; no unallowed vulnerabilities |
| V15 | `cargo deny check advisories bans sources` (cargo-deny 0.20.2) | PASS (exit 0) | advisories ok, bans ok, sources ok |
| V16 | `cargo deny check licenses` | PASS (exit 0) | licenses ok (blocking gate per deny.toml) |

## Frontend (frontend-ci.yml recipe)

| # | Command | Result | Detail |
|---|---|---|---|
| V3 | `npm ci --ignore-scripts` (apps/control-plane) | PASS (exit 0, 11 s) | deterministic install from package-lock.json |
| V4 | lockfile consistency check (CI's exact node script) | PASS | every package.json dependency matches the lockfile |
| V5 | `npm run lint` | PASS (exit 0, 4 s) | eslint: 0 errors, 22 warnings (warnings do not fail the CI gate) |
| V6 | `npm run typecheck` (`tsc --noEmit`) | PASS (exit 0, 2 s) | |
| V7 | `NEXT_TELEMETRY_DISABLED=1 npm run build` | PASS (exit 0, 13 s) | production build; all /trading routes prerendered |

## Release verification (ci.yml `release` job recipe — read-only checks)

| # | Command | Result |
|---|---|---|
| V17 | `scripts/verify-buyer-package.sh` | PASS |
| V18 | `scripts/verify-delivery.sh` | PASS (7 PASS / 0 FAIL) |
| V19 | `scripts/verify-release-integrity.sh` | PASS (parity, manifest counts, contamination, version 0.1.0) — first run FAILed on the exec-bit environment artifact, PASS after restore of `+x` |
| V20 | `scripts/forensic-sql-scan.sh` | PASS (0 class-4 findings) |
| V21 | `tests/business/business-matrix-completeness.sh` | PASS (7 lines × 11 columns, machine-matched) |
| V22 | `tests/release/marketing_claims.sh` | PASS (real tree clean; gate proven to reject planted violations) |
| V23 | `tests/release/manifest_current.sh` | PASS |
| V24 | `tests/release/buyer_parity.sh` | PASS |
| V25 | `tests/release/buyer_source_parity.sh` | PASS (parity holds; drift detection proven) |
| V26 | `tests/forensics/sql-pattern-regression.sh` | PASS |
| — | CI stale-manifest scan (docs vs release-manifest.json counts) | PASS (docs=122, rust=564, tests=1740 — matches the manifest exactly) |
| — | `scripts/generate-sbom.sh`, `scripts/generate-license-report.sh`, `scripts/build-release-package.sh` | NOT RUN (deliberate) | they regenerate committed artifacts (sbom*.json, licenses.*, buyer-release/) with fresh timestamps; the committed artifacts are already verified current (V17–V26). CI regenerates them on push. |

## Pre-existing repository inconsistencies FOUND and DOCUMENTED (not fixed —
this is a packaging task; the fix belongs to the next phase from the GitHub source)

1. **ci.yml migration gate is stale**: the step runs
   `ls crates/core/migrations/*.sql | wc -l | grep -q 24`, but the tree
   contains **35** migrations (high-water `0035`). The step FAILs on the
   current tree. The migrations themselves are monotonic 0001→0035 and
   correct; only the hard-coded count in the workflow is stale.
2. **ci.yml secret-scan allowlist is stale**: reproducing the release job's
   `grep -R "BEGIN PRIVATE KEY" crates docs | …` pipeline on the current
   tree leaves 6 surviving lines — all of them redaction implementation
   code (`crates/server/src/ops/external_evidence.rs:336`,
   `crates/server/src/ops/provider_contract_runner.rs:115`) and
   documentation describing the secret scan itself
   (`docs/BUILD-OUTPUT-HYGIENE-RESOLUTION.md:38`,
   `docs/EXTERNAL-VALIDATION-RUNBOOK.md` ×2,
   `docs/RELEASE-NOTES-CURRENT.md`). Zero of them contain key material
   (no base64 key bodies anywhere in the matches). The gate would
   false-positive-fail on GitHub until its allowlist is updated.
3. **`apps/control-plane/tsconfig.tsbuildinfo`** is a TypeScript build
   artifact covered by the `.gitignore` pattern `*.tsbuildinfo`, yet it
   exists in the working tree (and therefore in the buyer package's
   product-file count). It is EXCLUDED from this source ZIP as a build
   artifact; the next fix phase should decide whether to remove it from the
   working tree.

## Secret scan of the staged release tree

- CI's own secret-scan logic: 6 false-positive matches as documented above —
  **zero real private-key material**.
- Extended scan (private-key headers, `sk_live_`/`rk_live_`/`whsec_`,
  `AKIA…`, `ghp_…`, `github_pat_…`, `xox[baprs]-…`, Telegram bot-token
  pattern, secret-named files, embedded-password URLs): **PASS — no real
  secrets**. All 18 pattern matches are (a) redaction code, (b) deliberately
  fake unit-test fixtures (`sk_live_secret_12345`,
  `sk_live_very_secret_12345678`, `whsec_test_123`,
  `sk_live_51Hxxx_secret_value_long_enough_to_be_secret_xxxxxxxxxxxxxxxx`)
  in `#[cfg(test)]` modules — one of them is itself an assertion that debug
  output does NOT leak the key, (c) a SQL comment documenting that
  `key_prefix` is a public identifier, and (d) `.env.template` comments
  describing key FORMATS with empty values.
- Secret-named files absent: no `.env`, `*.pem`, `*keypair*.json`,
  `id.json`, `solana.json`, `*.key`, `dump.rdb` anywhere in the staged tree.
- `docker-compose.yml` uses env-var interpolation
  (`${POSTGRES_PASSWORD:?}`) — no literal credentials.

## Commands NOT RUN, and why (accurate terminology)

- `cargo build-sbf` + `STAKING_E2E=1 … validator_e2e` — UNAVAILABLE
  (Solana/Agave toolchain + test validator not installable in this
  sandbox; CI covers it).
- Docker image build + `docker compose config` gate — UNAVAILABLE (no
  Docker daemon/CLI in the sandbox; CI covers it).
- Live-provider gated tests (`LIVE_BILLING=1`, `LIVE_CUSTODY=1`,
  `DEPLOYMENT_BASE_URL=…`, `RPC_URL=…`) — NOT RUN, live-gated by design;
  they were never supposed to run during packaging (mirrors CI's isolated
  `external-gated` job; see docs/FINAL-BUYER-GAP-LEDGER.md GAP-001…006).
- External security audit (GAP-006) — NOT DONE (external auditor
  deliverable; never claimed).
- Funded trading (GAP-004) — NOT RUN (requires funded keys + explicit live
  approval; `funded_mode_guard` proves default-never-funded and DID run
  green inside V9b).
