# Buyer Verification Script — sniper-suite 0.1.0

> Single documented sequence. Every command corresponds to actual `scripts/*` and repo paths. Categories never mixed.

## Prerequisites

```bash
git clone <repo> sniper-suite && cd sniper-suite
cat VERSION  # 0.1.0
cat rust-toolchain.toml  # 1.98.1
```

## A. HERMETIC (no services, no secrets)

**Purpose:** Fast, deterministic, offline.

```bash
# 1) Format + build + lint (hard gates)
cargo fmt --all --check              # PASS
cargo check --workspace              # PASS (343 rs, 22 migrations, 8 members, 1.98.1)
cargo clippy --workspace --all-targets -- -D warnings  # PASS (per-crate in final-release-check)

# 2) Unit tests (hermetic, no PG/Redis)
cargo test -p saas-sdk -- --test-threads=1                    # 32/32 PASS
cargo test -p bot-core --lib -- --test-threads=1              # 507+ (2026-09-26 run: billing_state/dunning etc.)
cargo test -p sniper-suite --test observability_config -- --nocapture  # Batch5 harness 3/3
cargo test -p sniper-suite --test release_manifest_integration -- --nocapture
cargo test -p sniper-suite --test buyer_package_integration -- --nocapture
cargo test -p sniper-suite --test backup_restore_integration -- --nocapture
# Or all hermetic:
cargo test --workspace -- --test-threads=1 --skip validator_e2e  # 1331 grep, 32 saas-sdk, 105 server etc. (service-gated skipped as NOT_RUN)

# 3) Frontend hermetic
cd apps/control-plane
npm ci --ignore-scripts               # deterministic, 6171 lines v3
npm run typecheck                    # strict TS, noUncheckedIndexedAccess
npm run build                        # 5 routes prerendered static
npm run lint                         # eslint 9 flat config; exit 0, 12 documented warnings
cd ../..

# 4) Release hygiene hermetic
bash scripts/verify-delivery.sh      # 7/7 PASS (target/ INFO, excluded from package)
bash scripts/verify-buyer-package.sh # PASS (version 0.1.0, migrations 22, no secrets)
bash scripts/generate-sbom.sh        # → sbom.json 34758 B, sha fd837e42…
bash scripts/generate-license-report.sh # → licenses.json 107K, 707 entries
bash scripts/build-release-package.sh   # → buyer-release/ (7/7 checks, excludes target/node_modules/.git/.env)
cat buyer-release/checksums/SHA256SUMS  # per-artifact sha256+size+timestamp independently
```

**Expected:** All PASS. No external secrets needed.

## B. SERVICE-BACKED (Postgres + Redis)

**Purpose:** Tenant isolation, lifecycle, dedup/leases are `NOT_RUN` without services — provide them to verify.

```bash
# 1) Start services (Docker)
docker compose up -d postgres redis  # or postgres:16-alpine, redis:7-alpine as in ci.yml
export POSTGRES_URL=postgres://sniper:sniper@localhost:5432/sniper
export REDIS_URL=redis://localhost:6379

# 2) Run gated harnesses (each prints NOT_RUN if missing — here should PASS)
cargo test --test db_integration -- --test-threads=1 --nocapture          # 26/26 (bot-core)
cargo test --test redis_integration -- --test-threads=1 --nocapture       # 10/10
cargo test --test distributed_integration -- --test-threads=1 --nocapture # 4/4
cargo test --test postgres_saas_integration -- --test-threads=1 --nocapture # SaaS PG
cargo test --test redis_saas_integration -- --test-threads=1 --nocapture  # SaaS Redis
cargo test --test tenant_lifecycle_integration -- --test-threads=1 --nocapture
cargo test --test billing_integration -- --test-threads=1 --nocapture     # fixtures, not live

# 3) Release evidence with services (optional)
bash scripts/release-evidence.sh   # → release-evidence/summary.json (PASS/NOT_RUN per check)
```

**Expected:** PASS with services; without, `eprintln!("NOT_RUN: ... — POSTGRES_URL missing")` and `NOT_RUN` status — not a failure.

## C. EXTERNAL (requires buyer-provisioned secrets, not claimed)

**Purpose:** Document what would be needed to turn `EXTERNAL_REQUIRED` → `VERIFIED`. Never run without buyer secrets.

```bash
# Stripe/Paddle live (GAP-001) — example placeholders only
LIVE_BILLING=1 STRIPE_API_KEY=sk_live_example_... STRIPE_WEBHOOK_SECRET=whsec_example_... \
  cargo test --test live_billing_contract -- --ignored --nocapture   # NOT_RUN without LIVE_BILLING=1; never PASS in hermetic

# Vault/KMS/HSM live (GAP-002) — boundary verified, remote backend is NOT implemented in this build
LIVE_CUSTODY=1 VAULT_ADDR=https://vault.example.com VAULT_TOKEN=... \
  cargo test --test live_custody_contract -- --ignored --nocapture   # fail-closed: no local fallback; EXTERNAL_REQUIRED without creds

# Production deployment (GAP-003)
docker build -t sniper-suite:prod . && docker run -p 8080:8080 --env-file .env sniper-suite:prod  # LOCAL CONTAINER SMOKE only
curl -fsS http://localhost:8080/api/health | jq                                                   # local, never production proof
DEPLOYMENT_BASE_URL=https://your-host.example.com cargo test --test deployment_smoke -- --nocapture  # the actual GAP-003 check

# Funded trading (GAP-004) — paper is default; the funded step is operator-only
cargo test -p sniper-suite --lib funded_mode_guard          # guard: default never live/funded; live_unfunded denied
# live transition additionally requires EXECUTION_MODE=live + execution.allow_live_trading=true + owner approval + funded wallet
# (docs/LIVE-VALIDATION.md § GAP-004) — testnet/devnet success is NOT funded-live evidence

# Staking validator E2E (GAP-005) — excluded workspace; the cd is required; needs solana-test-validator + .so
cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1

# External audit (GAP-006) — no command; auditor deliverable (handover slot: docs/EXTERNAL-VALIDATION-RUNBOOK.md § GAP-006)
```

**Expected in this repo:** `external_validation.rs` defaults all to `NOT_EXECUTED`/`REQUIRES_EXTERNAL`; `final_gap_ledger.rs` lists 6 gaps with `EXTERNAL_REQUIRED`/`BUYER_ACTION`. No `VERIFIED` without execution.

## D. Full Release Check (combines A+B, skips C)

```bash
bash scripts/final-release-check.sh  # ALL PASS (hermetic; service-backed NOT_RUN locally)
bash scripts/final-release-check.sh  # run twice to show idempotent
bash scripts/build-release-package.sh && bash scripts/verify-buyer-package.sh && bash scripts/verify-delivery.sh
```

**Artifacts to inspect after:** `buyer-release/checksums/SHA256SUMS`, `sbom.json`, `licenses.json`, `release-manifest.json` (`docs_files:101`, `rust_files:343`, `test_count:1331`, `migrations:22`).

> **Separation guarantee:** HERMETIC never requires `POSTGRES_URL`/`REDIS_URL`/`STRIPE_API_KEY`/`VAULT_ADDR`; SERVICE-BACKED never claims external provider; EXTERNAL never runs on PR (see `.github/workflows/ci.yml` `external-gated` `if: workflow_dispatch`).
