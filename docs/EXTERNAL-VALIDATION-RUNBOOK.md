# External Validation Runbook — sniper-suite 0.1.0

> Buyer/operator runbook for all external validations. Each validation lists prerequisites, env vars **by NAME only** (no values), command, expected result, evidence files, failure interpretation, security precautions. Distinguishes **fixture vs service-backed vs live external vs production**.

## Modes

| Mode | What it tests | Requires | Live trade? | Command |
|---|---|---|---|---|
| **fixture** | Deterministic local fixtures (billing/custody success/failure/retry/duplicate, NON-LIVE label) | Nothing (hermetic) | Never | `cargo test --test provider_contracts -- --nocapture` `cargo test -p sniper-suite billing::live_provider_fixture` |
| **service-backed** | Postgres/Redis with real DB (migrations 0022, leases) | `POSTGRES_URL`, `REDIS_URL` | Never | `POSTGRES_URL=... REDIS_URL=... cargo test --test provider_contracts -- --nocapture` |
| **live external** | Live Stripe/Paddle, Vault/KMS/HSM, Solana RPC/Geyser, staking validator E2E | Live credentials + explicit flag | Only preflight guard, not auto-trade | `scripts/run-external-validation.sh {billing|custody|solana|staking}` with `LIVE_BILLING=1` etc. |
| **production** | Deployed `DEPLOYMENT_BASE_URL` health/readiness/OpenAPI/headers/frontend | `DEPLOYMENT_BASE_URL` | Never (read-only smoke) | `scripts/run-external-validation.sh deployment` or `cargo test --test deployment_smoke` |
| **all-safe** | Safe orchestration of all above without enabling live | None (defaults to NOT_RUN) | **Never** | `bash scripts/run-external-validation.sh all-safe` |

## 0. Canonical external gaps (GAP-001 … GAP-006)

Source of truth: `crates/server/src/ops/final_gap_ledger.rs` (`FinalGapLedger::default_ledger()`)
plus `docs/FINAL-BUYER-GAP-LEDGER.md`. The status registry
(`crates/server/src/ops/external_validation.rs`) maps every validation below to the same ids and
a test (`gap_mapping_matches_ledger`) fails if the two ever drift.

| GAP | Area | Owner | Truthful status (2026-09-27) | Verification command (exact) | Evidence file |
|---|---|---|---|---|---|
| GAP-001 | billing | Buyer | `EXTERNAL_REQUIRED` — adapter + end-to-end wiring verified; **live execution NOT_RUN** | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored --nocapture` | `evidence/external/billing_stripe.json` |
| GAP-002 | custody | Buyer | `EXTERNAL_REQUIRED` — real Vault-transit + AWS-KMS adapters implemented and unit-tested; the gap is the LIVE round-trip only (HSM additionally still unimplemented, fail-closed; see §2) | `LIVE_CUSTODY=1 VAULT_ADDR=... cargo test --test live_custody_contract -- --ignored --nocapture` | `evidence/external/custody_vault.json` |
| GAP-003 | deployment | Buyer | `BUYER_ACTION` — smoke harness verified, **NOT_RUN** (no URL tested) | `DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke -- --nocapture` | `evidence/external/deployment_deployment.json` |
| GAP-004 | trading | Buyer | `EXTERNAL_REQUIRED` — guard verified, **funded step operator-only** | `cargo test -p sniper-suite --lib funded_mode_guard` | `evidence/external/funded-preflight_funded.json` |
| GAP-005 | staking | External | `EXTERNAL_REQUIRED` — harness ready, **NOT_RUN/BLOCKED** (placeholder program id ⇒ BLOCKED) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | `evidence/external/staking_staking_validator.json` |
| GAP-006 | audit | External | `EXTERNAL_REQUIRED` — **no external audit performed** | n/a — external auditor deliverable (see § GAP-006) | none exists |

No row may become `VERIFIED` without executing its command in the required environment:
`set_status` refuses `VERIFIED` and `mark_verified(id, evidence_ref, verified_at, detail)`
requires a non-empty evidence file + timestamp.
`solana` (RPC/Geyser read-only) and the DB integrations are *not* one of the six gaps and carry
`gap_id: "n/a"` / empty respectively.

### 0.1 Canonical handover table — the six unresolved external items (frozen 2026-09-27)

**This is the single canonical handover table**; §0 above is the same six items as a compact status
summary, and §1–§6 below give the full narrative. Status vocabulary is exactly
`NOT_RUN` · `PASS` · `FAIL` · `BLOCKED` · `REQUIRES_EXTERNAL` · `VERIFIED`. A gap may move to
`VERIFIED` **only** after its exact command succeeds in the required environment and
`mark_verified(<id>, <evidence file>, <date>, <detail>)` records it — there is no default `VERIFIED`
and no manual “green” override.

| gap_id | area | current_status | owner | buyer/external prerequisite | exact environment variables | exact command | expected success evidence | expected credential-free result | evidence output path | status transition rule |
|---|---|---|---|---|---|---|---|---|---|---|
| GAP-001 | billing | `EXTERNAL_REQUIRED` (harness `NOT_RUN`) | Buyer | Stripe **or** Paddle account, live key, webhook secret, funded test account (see §1.1/§1.2) | `LIVE_BILLING`, `STRIPE_API_KEY`, `STRIPE_WEBHOOK_SECRET`, `SAAS_WEBHOOK_SECRET_STRIPE`, `PADDLE_API_KEY`, `PADDLE_WEBHOOK_SECRET`, `SAAS_WEBHOOK_SECRET_PADDLE` | `LIVE_BILLING=1 STRIPE_API_KEY=<redacted> STRIPE_WEBHOOK_SECRET=<redacted> cargo test --test live_billing_contract -- --ignored --nocapture` (Paddle: same with `PADDLE_API_KEY`/`PADDLE_WEBHOOK_SECRET`) | checkout session created at the provider, webhook HMAC verified, event processed idempotently, subscription synced — evidence JSON shows the boolean set true plus real provider ids | `EXTERNAL_REQUIRED` (missing API key) — typed 503, row stays `pending`, **no fabricated checkout session** | `evidence/external/billing_stripe.json` | `NOT_RUN → PASS` only from a real run; then `mark_verified("GAP-001", <evidence file>, <date>, <detail>)` |
| GAP-002 | custody | `EXTERNAL_REQUIRED` (harness `NOT_RUN`) | Buyer | Vault cluster (transit ed25519 key) or AWS KMS (`ECC_ED25519` key) reachable — the REAL adapters in `crates/server/src/custody/{vault,kms}/` sign through them; HSM cannot pass (unimplemented — fail-closed refusal naming the PKCS#11 dependency) | `LIVE_CUSTODY`, `VAULT_ADDR`, `VAULT_TOKEN`, `KMS_KEY_ID`, `HSM_SLOT`, `CREDENTIAL_REF` | `LIVE_CUSTODY=1 VAULT_ADDR=<redacted> VAULT_TOKEN=<redacted> cargo test --test live_custody_contract -- --ignored --nocapture` | remote sign available with a real key reference; evidence carries public-key metadata/ids only — never private material | `EXTERNAL_REQUIRED`; typed failure, **no local private-key fallback** | `evidence/external/custody_vault.json` | `NOT_RUN → PASS` only from a real remote-sign run; then `mark_verified("GAP-002", …)` |
| GAP-003 | deployment | `BUYER_ACTION` (harness `NOT_RUN`) | Buyer | deployed staging/production host with TLS + Postgres 16+ + Redis 7 (see §3 levels and `docs/BUYER-DEPLOYMENT.md`) | `DEPLOYMENT_BASE_URL`, `ENVIRONMENT`, `DEPLOYMENT_SMOKE_LIVE` | `DEPLOYMENT_BASE_URL=https://<real> cargo test --test deployment_smoke -- --nocapture` | `/api/health`, `/ready`, `/api/saas/openapi.json`, security headers and frontend all answer correctly with recorded latencies | `NOT_RUN` / `EXTERNAL_REQUIRED` — `localhost` is refused, no environment is invented | `evidence/external/deployment_deployment.json` | `NOT_RUN → PASS` only against a real base URL; then `mark_verified("GAP-003", …)`; local `docker run` never counts |
| GAP-004 | trading (funded) | `EXTERNAL_REQUIRED` (guard proven; funded step operator-only) | Buyer (operator) | explicit owner authorization, funded wallet, risk configuration, live-gate checks; supervised operator window per `docs/LIVE-VALIDATION.md` §2.3 | `EXECUTION_MODE`, `ALLOW_LIVE_TRADING`, `SOLANA_KEYPAIR`, `LIVE_GATE_*` | `cargo test -p sniper-suite --lib funded_mode_guard` (guard) and `bash scripts/run-external-validation.sh funded-preflight` (preflight record) | guard test PASS **plus** a supervised canary producing real order/fill records, audit-journal entries and reconciliation output | `NOT_RUN` — default `EXECUTION_MODE=dry_run` never auto-trades; `live_unfunded` denied by the guard; no funded trade is placed by any hermetic run | `evidence/external/funded-preflight_funded.json` | `NOT_RUN → PASS` only after the supervised operator run; then `mark_verified("GAP-004", …)` |
| GAP-005 | staking | `EXTERNAL_REQUIRED` (`NOT_RUN`/`BLOCKED` — placeholder program id) | External | agave 2.1.21 + `solana-test-validator` (or a real cluster), compiled `.so` (`cargo build-sbf`), buyer keypair `set-id`; placeholder `3vEEMM…` ⇒ `BLOCKED` | `RPC_URL`, `STAKING_PROGRAM_ID`, `STAKING_E2E`, `GEYSER_URL` (optional) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | validator E2E tests pass against a deployed program id, with the program/account identity (binary hash, slot) recorded | `EXTERNAL_REQUIRED`/`BLOCKED` (placeholder identity detected, fail-closed) | `evidence/external/staking_staking_validator.json` | `BLOCKED/NOT_RUN → PASS` only with real validator output; historical results never converted (check `is_current`) |
| GAP-006 | audit | `EXTERNAL_REQUIRED` (no report exists) | External | commissioned independent audit firm (deliverable, not a command) | none | n/a — deliverable handover slot (see § GAP-006) | signed audit report + findings register + remediation + retest + sign-off present in the buyer repo | n/a — stays `EXTERNAL_REQUIRED`; internal `cargo audit`/tests are **not** an external audit | `audit/report.pdf`, `audit/findings.json`, `audit/remediation/*`, `audit/retest/*`, `audit/signoff.pdf` or `audit/signoff.md` (buyer repo) | `EXTERNAL_REQUIRED → VERIFIED` only after sign-off exists; then `mark_verified("external_audit", <signoff path>, <date>, <detail>)` |

## General

- **Evidence dir:** `evidence/external/` (default, set `EVIDENCE_DIR` to override). Each run saves `validation_id_provider.json` with `validation_id, gap_id, provider, environment, timestamp, command (redacted), mode, status (PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED/BLOCKED), evidence_hash, redacted_metadata, endpoint_ref`. Secrets never persisted (redacted to `<redacted>`).
- **Evidence hash (canonical + deterministic):** `evidence_hash` = SHA256 of the compact JSON
  `{"command":…,"endpoint_ref":…,"environment":…,"gap_id":…,"mode":…,"provider":…,"redacted_metadata":…,"status":…,"validation_id":…}`
  with keys sorted. `timestamp` is **excluded** on purpose: the same input tree + same command +
  same canonicalization always produce the same hash (timestamps remain a separate informational
  field). The rule is implemented identically in `crates/server/src/ops/external_evidence.rs`
  and `scripts/run-external-validation.sh`; both assert the same fixture digest
  (`1e3bf39a6d142631318d8b4d2f9da5072a2747935ced440194a20074e4e9c1b3`), and
  `cargo test --test provider_contracts` verifies every emitted file (hash + schema).
- **Status semantics:** `PASS` only with real evidence, `FAIL` on error/timeout/network, `NOT_RUN` when not enabled, `EXTERNAL_REQUIRED` when credentials missing, `BLOCKED` when placeholder (e.g., staking `3vEEMM…`).
- **No secret logging:** `DATABASE_URL`, `POSTGRES_URL`, `REDIS_URL`, `STRIPE_API_KEY`, `STRIPE_WEBHOOK_SECRET`, `PADDLE_API_KEY`, `PADDLE_WEBHOOK_SECRET`, `VAULT_TOKEN`, `VAULT_NAMESPACE`, `KMS_KEY_ID/SECRET`, `HSM_SLOT/PIN`, `SOLANA_KEYPAIR`, `WALLET_PRIVATE_KEY`, `SEED_PHRASE`/`MNEMONIC`, `Authorization: Bearer …` and `BEGIN PRIVATE KEY` never appear in logs/JSON/evidence (redaction in `redact_command` + `redact_metadata`, shell `redact()`; regression tests in `ops::external_evidence`).

## 1. Billing — Live Stripe/Paddle

**Prerequisites:** Stripe or Paddle account, API key, webhook secret, funded test account. **Do NOT use `if LIVE_BILLING=1 { return payment_succeeded; }` — must perform real provider operation.**

| Item | Value |
|---|---|
| **Env vars (NAME only)** | `LIVE_BILLING`, `STRIPE_API_KEY`, `STRIPE_WEBHOOK_SECRET`, `PADDLE_API_KEY`, `PADDLE_WEBHOOK_SECRET` |
| **Command (live)** | `LIVE_BILLING=1 STRIPE_API_KEY=<redacted> STRIPE_WEBHOOK_SECRET=<redacted> cargo test --test live_billing_contract -- --ignored --nocapture` |
| **Via runner** | `LIVE_BILLING=1 STRIPE_API_KEY=<redacted> bash scripts/run-external-validation.sh billing` |
| **Expected (without creds)** | `EXTERNAL_REQUIRED` (missing API key) |
| **Expected (with creds, hermetic)** | `NOT_RUN` (live execution not performed without real provider call; real live would create checkout session, verify HMAC, process event, check idempotency) |
| **Evidence** | `evidence/external/billing_stripe.json` — `provider=stripe, credentials_configured, signature_verified, checkout_created, event_processed, idempotency_verified, subscription_synced (all redacted, only booleans + ids)` |
| **Failure** | `FAIL` on timeout/network/provider error (distinguishable: `TIMEOUT` vs `NETWORK_UNREACHABLE` vs `PROVIDER_ERROR`) |
| **Fixture (non-live)** | `cargo test --test provider_contracts` + `billing::live_provider_fixture` — success/failure/retry/duplicate (label `NON-LIVE FIXTURE`) — not live evidence |
| **Adapter + wiring status** | `ADAPTER + END-TO-END WIRING READY / LIVE NOT_RUN`. `BillingService::create_checkout` resolves `provider_registry` → adapter → provider request → durable `provider_session_id` + `checkout_url` + status `open`; `Idempotency-Key` propagated; without `LIVE_BILLING=1` it returns typed `NOT_RUN` (503) and leaves the row `pending` — no fabricated session id, state or URL |
| **PostgreSQL (service-backed)** | `POSTGRES_URL=postgres://… cargo test --manifest-path crates/server/Cargo.toml --bins checkout_pg_durable_idempotency checkout_provider_success invoice_pg_durable_reads -- --test-threads=1` — **SERVICE-BACKED VERIFIED** against real PostgreSQL 17.11 (migrations 0001–0022) |
| **Security** | Never log `sk_live`, webhook secret; `saas-sdk` secret-free Debug |

### 1.1 Stripe handover (account, credentials, webhook, artifacts, cleanup)

| Item | Value |
|---|---|
| **Required account** | Stripe account with live-mode access; a funded test customer/price usable for a real checkout (test-mode keys exercise the same code path and are acceptable for the harness run; record which mode you used) |
| **Required credentials** | `STRIPE_API_KEY` (secret key), `STRIPE_WEBHOOK_SECRET` (endpoint signing secret) — **by name only here; never commit values** |
| **Required webhook configuration** | Register `POST https://<your-host>/api/saas/billing/webhooks/stripe` (subscription lifecycle) and `POST https://<your-host>/api/saas/billing/payment-webhooks/stripe` (payment/invoice events — both are `:provider` routes in `crates/server/src/saas/billing_webhook.rs` / `crates/server/src/saas/payment_webhooks.rs`); subscribe at least `checkout.session.completed`, `payment_intent.succeeded`, `invoice.paid`. Runtime signature verification reads **`SAAS_WEBHOOK_SECRET_STRIPE`**; the live harness reads **`STRIPE_WEBHOOK_SECRET`** — both must hold the endpoint's signing secret |
| **Required environment variables** | `LIVE_BILLING=1`, `STRIPE_API_KEY`, `STRIPE_WEBHOOK_SECRET` (harness), `SAAS_WEBHOOK_SECRET_STRIPE` (runtime webhook verification), plus `DEPLOYMENT_BASE_URL` if you want the smoke harness to reach the endpoint |
| **`LIVE_BILLING` requirement** | Without `LIVE_BILLING=1` the service returns a typed `NOT_RUN` (503) and leaves the checkout row `pending` — no fabricated session id, state or URL |
| **Exact test command** | `LIVE_BILLING=1 STRIPE_API_KEY=<redacted> STRIPE_WEBHOOK_SECRET=<redacted> cargo test --test live_billing_contract -- --ignored --nocapture` |
| **Expected provider-side artifact** | A real Checkout Session / PaymentIntent id visible in the Stripe dashboard, and a delivered webhook event (event id) for the endpoint above |
| **Expected repository evidence** | `evidence/external/billing_stripe.json` with `credentials_configured`, `signature_verified`, `checkout_created`, `event_processed`, `idempotency_verified`, `subscription_synced` true and the provider ids recorded (redacted metadata only) |
| **Cleanup requirement** | After the run, cancel/expire the test subscription and refund or void the test charge in Stripe so no live money movement remains; keep the webhook event ids with your evidence |
| **Without credentials** | `EXTERNAL_REQUIRED` (missing API key) — safe, non-fabricated; the harness never returns a success path without a real provider call |

### 1.2 Paddle handover (account, credentials, webhook, artifacts, cleanup)

| Item | Value |
|---|---|
| **Required account** | Paddle account (sandbox or live) with a product/price and a usable checkout |
| **Required credentials** | `PADDLE_API_KEY`, `PADDLE_WEBHOOK_SECRET` — by name only |
| **Required webhook configuration** | Register `POST https://<your-host>/api/saas/billing/webhooks/paddle` (subscription lifecycle) and `POST https://<your-host>/api/saas/billing/payment-webhooks/paddle` (payment events) for at least `transaction.completed` and `subscription.updated`. Runtime signature verification reads **`SAAS_WEBHOOK_SECRET_PADDLE`**; the live harness reads **`PADDLE_WEBHOOK_SECRET`** |
| **Required environment variables** | `LIVE_BILLING=1`, `PADDLE_API_KEY`, `PADDLE_WEBHOOK_SECRET` (harness), `SAAS_WEBHOOK_SECRET_PADDLE` (runtime webhook verification) |
| **`LIVE_BILLING` requirement** | Same gate as Stripe — no live execution without `LIVE_BILLING=1` |
| **Exact test command** | `LIVE_BILLING=1 PADDLE_API_KEY=<redacted> PADDLE_WEBHOOK_SECRET=<redacted> cargo test --test live_billing_contract -- --ignored --nocapture` |
| **Expected provider-side artifact** | A real transaction id / subscription id visible in the Paddle dashboard and a delivered notification (event id) |
| **Expected repository evidence** | `evidence/external/billing_stripe.json` (shared billing record) with the Paddle provider fields set and the ids recorded (redacted) |
| **Cleanup requirement** | Cancel the sandbox subscription / refund the test transaction so no recurring charge remains; keep the event ids with your evidence |
| **Without credentials** | `EXTERNAL_REQUIRED` — identical fail-safe behaviour to Stripe; no Paddle call is simulated |

## 2. Custody — Live Vault/KMS/HSM

**Prerequisites:** Vault cluster (`VAULT_ADDR`, `VAULT_TOKEN`) or KMS (`KMS_KEY_ID`) or HSM (`HSM_SLOT`), remote provider selected → remote required, **no local fallback**, no private key export, no secret logging.

> **Build truth (read before provisioning):** the custody *boundary* — provider selection,
> credential-reference validation and fail-closed refusal — is implemented and tested. The
> **remote signing backends themselves are not implemented in this build**: selecting
> `vault`/`kms`/`hsm` as the signer fails startup with `SignerError::UnsupportedBackend`
> (`crates/solana-kit/src/signer.rs`, `docs/SECURITY.md § Key management`) instead of silently
> falling back to a local key. GAP-002 therefore stays `EXTERNAL_REQUIRED` until a backend is
> implemented *and* a real cluster is provisioned; the commands below verify the boundary and
> the fail-closed contract, and never claim a production remote sign.

| Env vars | `LIVE_CUSTODY`, `VAULT_ADDR`, `VAULT_TOKEN`, `KMS_KEY_ID`, `HSM_SLOT` |
|---|---|
| **Command (live)** | `LIVE_CUSTODY=1 VAULT_ADDR=<redacted> VAULT_TOKEN=<redacted> cargo test --test live_custody_contract -- --ignored --nocapture` |
| **Via runner** | `LIVE_CUSTODY=1 VAULT_ADDR=<redacted> bash scripts/run-external-validation.sh custody` |
| **Expected (without creds)** | `EXTERNAL_REQUIRED` (no local fallback) |
| **Expected (with creds, hermetic)** | `NOT_RUN` (credentials valid but live remote sign not performed without network) |
| **Evidence** | `evidence/external/custody_vault.json` — `provider=vault, credential_valid, sign_available, public_key (redacted 8 chars), address, no private key` |
| **Failure** | `FAIL/CANNOT_SIGN` if provider unavailable, `BLOCKED` if revoked, `TIMEOUT` on network |
| **Fixture** | `custody::live_provider_fixture` — unavailable/unauthorized/success/revoked/timeout (all `NON-LIVE`) |
| **Key reference requirements** | `CREDENTIAL_REF=env_var:<NAME>` (or the provider-native reference) only — the evidence record stores the **reference**, never the material; a missing/unresolvable reference fails closed |
| **Rotation expectations** | `crates/core/src/custody/rotation.rs` enforces `Pending → Active → Draining → Revoked`, each transition explicit; rotate on a schedule you define, and after any suspected exposure. A rotation must be staged (`Pending`) before promotion — it is never implicit |
| **Failure contract (explicit)** | `REMOTE FAILURE → NO LOCAL PRIVATE-KEY FALLBACK → TYPED FAILURE`: a provider error/timeout/revocation surfaces as `FAIL`/`CANNOT_SIGN`/`BLOCKED`/`TIMEOUT`, the signer refuses the request, and the custody boundary refuses unconfigured/unreachable remote providers with typed errors naming the exact dependency (`VAULT_TRANSIT_DEPENDENCY` / `AWS_KMS_DEPENDENCY`); HSM selection refuses with its PKCS#11 dependency. (The separate solana-kit transaction-signer layer still fails startup with `SignerError::UnsupportedBackend` for `[signing] provider = vault/kms/hsm`.) No workflow in this repository ever exports private material (no `solana-keygen`-style export, no key dump, no plaintext key in env evidence) |
| **Security** | Evidence only public metadata/reference IDs/hash; `BEGIN PRIVATE KEY` never |

## 3. Deployment Smoke — Production/Staging

**Prerequisites:** Deployed URL (`DEPLOYMENT_BASE_URL`), optional version check, frontend if deployed.

**Deployment levels (do not conflate):**

| Level | What success proves | How |
|---|---|---|
| 1. Local container smoke | The image builds and the binary serves HTTP on your machine. **Never production verification.** | `docker build -t sniper-suite:prod . && docker run -p 8080:8080 --env-file .env sniper-suite:prod` then `curl -s localhost:8080/api/health` |
| 2. Buyer staging smoke | A staging deployment (host/TLS/Postgres/Redis) answers health, readiness, OpenAPI and security headers correctly | `DEPLOYMENT_BASE_URL=https://staging... cargo test --test deployment_smoke -- --nocapture` |
| 3. Production deployment | The real host is deployed and smoke-checked **by the operator**; only this counts as production verification | Same command against the production URL, run by the operator during the handover window |

A local `docker run` + `curl localhost` must never be recorded as GAP-003 verification; the
harness refuses to substitute `localhost` for a real `DEPLOYMENT_BASE_URL` (guarded by a test).

| Env vars | `DEPLOYMENT_BASE_URL`, `ENVIRONMENT`, `DEPLOYMENT_SMOKE_LIVE` (opt-in for live fetch) |
|---|---|
| **Command** | `DEPLOYMENT_BASE_URL=https://... cargo test --test deployment_smoke -- --nocapture` |
| **Via runner** | `DEPLOYMENT_BASE_URL=https://... bash scripts/run-external-validation.sh deployment` |
| **Checks** | `GET {base}/api/health`, `GET {base}/ready`, `GET {base}/api/saas/openapi.json`, migration state from health payload, Redis readiness where required, `CORS/security headers` (`CSP/XCTO/etc.`), frontend `GET {base}/` if `check_frontend` |
| **Expected (without URL)** | `NOT_RUN`/`EXTERNAL_REQUIRED` — **never claim deployment occurred when no URL was tested** |
| **Expected (with URL, hermetic)** | `NOT_RUN` with checks `EXTERNAL_REQUIRED` (real fetch requires `DEPLOYMENT_SMOKE_LIVE=1` + network) |
| **Evidence** | `evidence/external/deployment_deployment.json` — `base_url (redacted query), overall_status, checks[] with latency_ms, http_status, hash` |
| **Failure** | `FAIL` on timeout/network/4xx/5xx (distinguishable) |
| **Security** | URL query redacted to `?<redacted>` |

## 4. Solana — RPC/WS/Geyser (read-only, no trading)

**Prerequisites:** Solana RPC URL, optional WS, Geyser URL, `SOLANA_LIVE` for live.

| Env vars | `RPC_URL`, `WS_URL`, `GEYSER_URL`, `SOLANA_LIVE` |
|---|---|
| **Command** | `RPC_URL=https://... cargo test --test solana_contract -- --nocapture` |
| **Via runner** | `RPC_URL=https://... bash scripts/run-external-validation.sh solana` |
| **Checks** | RPC health, `getSlot`, `getHealth`, WS reachable, Geyser subscription/message decode, latency, slot/health where safe |
| **Expected (without URL)** | `EXTERNAL_REQUIRED` |
| **Expected (with URL, hermetic)** | `NOT_RUN` (endpoint configured but not tested without network) |
| **Evidence** | `evidence/external/solana_solana_rpc.json` — `rpc_reachable, ws_reachable, authenticated, latency_ms, slot, health, redacted_endpoint` |
| **Failure** | `FAIL` on `TIMEOUT`/`NETWORK_UNREACHABLE` |
| **Security** | URL credentials redacted (`user:pass@` → `<redacted>@`), no trading, no customer tx |

## 5. Staking — Deployment + Validator E2E

**Prerequisites:** RPC URL, program ID (not placeholder `3vEEMM…`), binary hash if configured, `STAKING_E2E=1` + `solana-test-validator` + `cargo build-sbf` toolchain agave 2.1.21.

| Env vars | `RPC_URL`, `STAKING_PROGRAM_ID`, `STAKING_E2E`, `GEYSER_URL` (optional) |
|---|---|
| **Compiled program (.so) requirement** | The E2E tests need the **compiled** program: `cargo build-sbf` (inside `programs/staking-suite`) produces `target/deploy/*.so`; deploying it (`solana program deploy`, or `solana-test-validator --bpf-program` for a local validator) is what makes the program id executable. A source tree without the `.so` cannot satisfy the validator step |
| **Deployment check** | `RPC_URL=https://... STAKING_PROGRAM_ID=... cargo test --test staking_contract -- --nocapture` → `deployment_contract.rs` verifies program exists, executable, binary hash, slot |
| **Validator E2E** | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` (3 tests, existing `validator_e2e.rs`) → requires validator available. `programs/staking-suite` is its own (excluded) workspace, so the `cd` is required; the placeholder program id `3vEEMM…` must never be read as a deployed program |
| **Via runner** | `STAKING_E2E=1 bash scripts/run-external-validation.sh staking` |
| **Expected (no config)** | `EXTERNAL_REQUIRED`/`BLOCKED` (placeholder → BLOCKED) |
| **Expected (with config, hermetic)** | `NOT_RUN` (not executed without real RPC/validator) |
| **Evidence** | `evidence/external/staking_staking_validator.json` — `e2e_executed, tests_passed/failed, validator_version, hash` |
| **Historical** | Never convert historical validator evidence into current PASS (check `is_current` flag) |

## 6. Funded Live-Trading Preflight (guard, no auto-trade)

**Prerequisites:** `EXECUTION_MODE`, `ALLOW_LIVE_TRADING`, wallet configured/funded, owner/admin, risk gates, emergency stop, audit, custody/provider readiness.

| Env vars | `EXECUTION_MODE`, `ALLOW_LIVE_TRADING`, `SOLANA_KEYPAIR` (redacted), `LIVE_GATE_*` |
|---|---|
| **Command** | `cargo test -p sniper-suite --lib funded_mode_guard` (guard: default never funded/live) or `bash scripts/run-external-validation.sh funded-preflight` (writes the evidence file) |
| **Checks** | `funded_mode_guard.rs` detects `simulate/paper/dry_run/live_unfunded/live_funded`; `live_gate.rs` validates enablement/environment/owner/provider/custody/audit/risk/emergency-stop — **fail closed** |
| **Expected (default)** | `paper`/`dry_run` → `NOT_RUN` (guard ready, execution is operator action) |
| **Expected (live but unfunded)** | `FAIL`/`EXTERNAL_REQUIRED` (wallet not funded etc.) |
| **Owner authorization** | Required before any funded step: an explicit owner/admin authorization (and the `live_gate` owner check) — the guard denies funded mode without it (`owner not authorized — funded mode denied`). Default state stays `EXECUTION_MODE=dry_run`; funded-live validation is a **buyer/external action**, never a seller-side run |
| **Post-trade evidence** | After a supervised canary the operator keeps: the preflight record (`evidence/external/funded-preflight_funded.json`), the audit-journal entries, the order/fill records and the reconciliation output for the canary window — that set, not the guard test, is what a funded `PASS` cites |
| **Evidence** | `evidence/external/funded-preflight_funded.json` — `execution_mode, allow_live_trading, wallet_configured/funded (booleans only, no private key), live_funded bool, hash` |
| **Security** | `FundedModeGuard::never_expose_private_key` → `<redacted>`; this batch **MUST NOT** execute funded live trades automatically |

## 7. All-Safe

```bash
bash scripts/run-external-validation.sh all-safe
# → never place funded trade, never expose private keys, never silently enable live payment/remote signing
# → prints PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED per validation, redacted, saves evidence under evidence/external/
# → without live env, all checks are NOT_RUN/EXTERNAL_REQUIRED (no fake PASS)
# → fails closed if an expected evidence file/field is missing
# → evidence_hash is canonical (timestamp excluded) and stable across runs
```

## Evidence Verification

```bash
# Hermetic canonical statuses + verifies every evidence/external/*.json file (hash + schema):
cargo test --test provider_contracts
cargo test --test deployment_smoke
cargo test --test solana_contract
cargo test --test staking_contract
# Live (ignored, require opt-in):
LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored --nocapture
LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored --nocapture
# Inspect a saved record:
python3 -c "import json; e=json.load(open('evidence/external/billing_stripe.json')); print(e['gap_id'], e['status'], e['evidence_hash'])"
# Hash/schema unit layer (canonicalization fixture + tamper detection):
cargo test -p sniper-suite --lib external_evidence
```
If a file was edited by hand the hash no longer matches and `provider_contracts` fails — that is
the tamper detector, not a flake.

## GAP-006 — External security audit handover slot

No external audit exists. The repository's own wording is internal-only: `cargo audit` /
`cargo deny` (best-effort, tool may be unavailable), the test suites, and the static gates are
evidence of *internal review* — nothing in this repo may be described as "externally audited",
"independently audited", "security-certified", "penetration-tested" or "formal audit complete"
until a real third-party report exists.

When the buyer commissions an audit, hand over exactly these artifacts (this is the agreed slot;
nothing is fabricated here):

| Artifact | Path convention | Required content |
|---|---|---|
| Audit report | `audit/report.pdf` (buyer repo) | scope, commit/version audited, methodology, date |
| Findings register | `audit/findings.json` | one entry per finding: `id, severity (critical/high/medium/low/info), component, description, reproduction` |
| Remediation evidence | `audit/remediation/<finding-id>.md` | fix commit, test added, evidence command + result |
| Retest evidence | `audit/retest/<finding-id>.md` | retest date, auditor verdict (`fixed` / `accepted-risk` / `open`) |
| Final sign-off | `audit/signoff.pdf` or `audit/signoff.md` | auditor name/entity, date, residual-risk statement |

Machine-readable status stays `EXTERNAL_REQUIRED` / `NOT_EXECUTED` in the registry
(`external_audit`) until the sign-off exists; then `mark_verified("external_audit", <signoff path>, <date>, ...)`.
`docs/SECURITY.md` and root `SECURITY.md` must keep saying "no external audit performed" until then.

## Security Precautions (all modes)

- No `DATABASE_URL`, `REDIS_URL`, `STRIPE_API_KEY`, `VAULT_TOKEN`, `SOLANA_KEYPAIR`, `BEGIN PRIVATE KEY` in logs/JSON — `redact_command`, `redacted_metadata`, `is_secret_like`.
- `evidence/` is `.gitignore`'d; commit only if intentionally publishing redacted evidence.
- Live tests are `#[ignore]` + explicit env flag + credentials + endpoint + expected environment — ordinary `cargo test --workspace` cannot activate live behavior (checked in `network_policy.rs` `prevent_accidental_live_in_tests`).
- Staking validator E2E never claims historical results; deployment never claims without RPC evidence.
- Billing never does `if LIVE_BILLING=1 { return payment_succeeded }` — must perform real configured provider operation.

## Expected Evidence Files (after all-safe with no live env)

```
evidence/external/
  billing_stripe.json      # NOT_RUN
  custody_vault.json       # NOT_RUN
  deployment_deployment.json # NOT_RUN
  solana_solana_rpc.json   # NOT_RUN
  staking_staking_validator.json # NOT_RUN
  funded-preflight_funded.json # NOT_RUN
# Each: validation_id, gap_id, provider, environment=test, timestamp, command (redacted), mode, status, evidence_hash (sha256, canonical), redacted_metadata, endpoint_ref
# gap_id: billing=GAP-001, custody=GAP-002, deployment=GAP-003, funded-preflight=GAP-004, staking=GAP-005, solana=n/a
```

See also: `docs/BUYER-TRUTH-REGISTER.md`, `docs/FINAL-BUYER-GAP-LEDGER.md`, `docs/BUYER-EVIDENCE-PACK.md` for truthful current external state (all NOT_RUN until buyer provides infrastructure).
