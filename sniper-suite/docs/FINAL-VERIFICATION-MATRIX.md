# Final Verification Matrix — sniper-suite 0.1.0

> 2026-09-26 Asia/Dhaka · `1.98.1` · `343 rs` · `101 docs` · `22 migrations` · `1331 tests` · `0.1.0`
> Each NOT_RUN includes exact reason + command + evidence. Historical never substituted.

| Check | Status | Command | Evidence | Reason if NOT_RUN |
|-------|--------|---------|----------|-------------------|
| **cargo fmt** | **PASS** | `cargo fmt --all -- --check` | exit 0, 0 diff | — |
| **cargo check** | **PASS** | `cargo check --workspace` | FINISHED 3m12s, 8 crates | — |
| **sniper-suite lib** | **PASS** | `cargo test -p sniper-suite --lib` | 263/263 PASS (3 ignored) — Batch 10 2026-09-27 | — |
| **saas-sdk** | **PASS** | `cargo test -p saas-sdk -- --test-threads=1` | 32/32 PASS 0.01s | — |
| **per-crate Clippy** | **PASS** | `bash scripts/final-release-check.sh` `[3/8]` per-crate `cargo clippy -p <crate> --all-targets -- -D warnings` | 8/8 crates PASS (sniper-suite 37s, saas-sdk 7s, bot-core 64s, solana-kit 55s, module-* 4–69s) | — |
| **Clippy per-crate sniper-suite** | **PASS** | `cargo clippy -p sniper-suite --all-targets -- -D warnings` | 0 warnings, exit 0 | — |
| **Clippy per-crate saas-sdk** | **PASS** | `cargo clippy -p saas-sdk --all-targets -- -D warnings` | 0 TRUE CLEAN | — |
| **workspace Clippy** | **PASS (2026-09-26)** | `cargo clippy --workspace --all-targets -- -D warnings` | **0 errors, 0 warnings** — passes with `CARGO_BUILD_JOBS=1 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0`. The earlier `SIGKILL 9 OOM` was a sandbox memory/disk limit, cleared by reducing debuginfo and build parallelism (no lint was relaxed). | **Workspace-wide PASS claimed with the stated environment settings.** |
| **full workspace tests** | **PASS (2026-09-26)** | `POSTGRES_URL=… cargo test --workspace -- --test-threads=1` with `CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1` | **70 suites, 2077 passed, 0 failed, 13 ignored** against real PostgreSQL 17.11, exit 0 | **Executed in the sandbox; no historical totals substituted.** |
| **PostgreSQL** | **SERVICE-BACKED VERIFIED (2026-09-26)** | `POSTGRES_URL=postgres://postgres:***@localhost:5432/sniper_test cargo test --workspace -- --test-threads=1` | Real PostgreSQL **17.11** with migrations `0001`–`0022` applied: `db_integration` 26/26, `postgres_saas_integration` 7/7, checkout durability/concurrency, provider-success persistence and invoice reads all executed and passing | Real DB ran in this sandbox; hermetic `NOT_RUN` behaviour remains the default when `POSTGRES_URL` is absent |
| **Redis** | **NOT_RUN** | `REDIS_URL=redis://host:6379 cargo test --test redis_integration` | harness READY, `NOT_RUN` without URL | No real Redis |
| **frontend typecheck** | **PASS** | `cd apps/control-plane && npm run typecheck` | `tsc --noEmit` exit 0 | — |
| **frontend build** | **PASS** | `npm run build` | `next build` (Next.js 16) 5 routes prerendered static, exit 0 | — |
| **frontend lint** | **PASS** | `npm run lint` (`eslint .`, ESLint 9 flat config) | exit 0, non-interactive, 12 pre-existing warnings (documented, row 16 of `KNOWN-LIMITATIONS.md`) | Batch 11 2026-09-27 |
| **delivery verification** | **PASS** | `bash scripts/verify-delivery.sh` | 7/7 PASS (75 files, migrations 22, version 0.1.0, docs 101, hygiene, links) | — |
| **buyer package verification** | **PASS** | `bash scripts/verify-buyer-package.sh` | PASS (VERSION/Cargo.lock/manifest/sha, no private key, WARN .next not shipped) | — |
| **release check** | **PASS** | `bash scripts/final-release-check.sh` | 8/8 ALL PASS (fmt/check/per-crate clippy/saas-sdk test/integration existence/secret/manifest/sbom/buyer/delivery) | — |
| **external all-safe** | **6/6 NOT_RUN** | `bash scripts/run-external-validation.sh all-safe` | all checks `NOT_RUN`/`EXTERNAL_REQUIRED`, redacted `evidence/external/*.json` SHA256, no secrets, no funded trade | **Expected with no credentials: NOT_RUN, not PASS** |

> `WARN .next present (not shipped)` and `resource_limited` are distinct from PASS. Historical `962/966` never substituted for current `NOT_RUN`.
