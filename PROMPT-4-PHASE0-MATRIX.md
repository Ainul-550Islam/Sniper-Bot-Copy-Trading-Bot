# PROMPT 4/10 — PHASE 0 FORENSIC MATRIX (pre-coding audit)

**Audit date:** 2026-09-30 (Asia/Dhaka)
**Source tree:** `/home/user/Sniper-Bot-Copy-Trading-Bot` (canonical; `rust_files = 520`, `migrations = 34`, `docs = 101` — verified against the tree and `release-manifest.json`)
**Baseline document:** current `AUDIT.md` (P0 findings 1–12 reproduced in §1 of `PROMPT-4-RESULT.md`).

Classification vocabulary (per prompt):

| code | meaning |
|---|---|
| **A** | complete |
| **B** | partial |
| **C** | missing |
| **D** | stale / synthetic |
| **E** | intentional global (operator/deployment scope, by design) |
| **F** | requires wiring |
| **G** | requires new migration |
| **H** | requires external evidence |

Every search the prompt mandates was executed on the current tree; no
finding is silently discarded. Counts are non-test source unless noted.

---

## 1. Tenant identity inside the module crates and execution paths

`grep OrganizationId | TenantExecutionContext | TenantExecutionGateway | TenantRuntime | TenantContext | organization_id` in `crates/module-{sniper,copy,polymarket,telegram}/src`, `crates/solana-kit/src`, server execution paths:

| finding | location | status | action |
|---|---|---|---|
| Sniper tenant identity (OrganizationId, TenantExecutionContext, TenantContext) | `module-sniper/src/tenant_context.rs`, `tenant_executor.rs` (+ `lib.rs` fields) | **A** | shipped this prompt (§B files 8–11) |
| Copy tenant identity | `module-copy/src/tenant_context.rs`, `tenant_executor.rs`, `tenant_state.rs` | **A** | shipped this prompt (§C files 12–15) |
| Polymarket tenant identity | 0 files | **C** | §D files 16–17 (this prompt) |
| Telegram tenant routing | 0 files (`module-telegram/src`: alerts/api/commands only) | **C** | business-matrix item; no file slot in the prompt's tree — recorded as remaining P1 in §15 of the result doc; the module factory's telegram arm fails closed `module_not_wired` until it exists |
| Solana-kit tenant boundary | `solana-kit/src/tenant_signing_context.rs`, `tenant_transaction.rs`, `tenant_broadcast_guard.rs` | **A** | shipped this prompt (§E files 26–29) |
| `TenantExecutionGateway` absent from module crates | by design: issuance lives server-side (`server/src/tenant/gateway.rs`); modules only consume issued contexts | **E** | none — correct layering |
| `TenantRuntime` type absent | the runtime identity type is `TenantRuntimeRecord` (`server/src/runtime_registry/model.rs`); no per-tenant MODULE instance bridge exists | **C → F** | §A files 1–7 (this prompt) |
| Server execution path tenant-blindness | `main.rs spawn_modules` builds process-global Sniper/CopyBot for the deployment organization | **E** for operator mode + **F** for tenant mode | §A factory; operator path must keep working unchanged |

## 2. Raw SQL against trading-truth tables

`INSERT INTO orders | UPDATE … | SELECT … FROM … | DELETE FROM …` for orders, executions, transactions, positions, trades, balance_snapshots, execution_intents, execution_claims, execution_lifecycle, copy_leaders, copy_events, copy_links, poly_signals, poly_orders, poly_fills, poly_recon_findings — searched in `crates/server/src`, all module crates, `crates/solana-kit/src` (non-test):

| finding | location | status | action |
|---|---|---|---|
| `INSERT INTO orders … ON CONFLICT (id)` (1 site) | `server/src/trading_data_plane/mod.rs:275` | **A** | inside `#[tokio::test]` seed code — test-only, org-bound insert |
| All other raw table SQL lives in repositories | `core/src/db/repo.rs` (legacy deployment plane) and `core/src/trading_repository/**` (tenant plane) | **A / E** | tenant repositories carry `organization_id` in every predicate (PROMPT 3, 727-test live-PG suite green this prompt); legacy repo is the operator plane (deployment organization) — intentional global, explicitly privileged |
| `WHERE id = $1` unqualified (4 sites) | `server/src/runtime_registry/store.rs:182/195/215` | **E** | `tenant_runtimes` registry rows are runtime-instance rows (control plane); queries by registry PK, org carried by the record — intentional |
| `WHERE id = $1` in `core/src/db/tenant_query.rs:112` | test asserting the tenant-blind form is **rejected** | **A** | guard test, production-safe |

## 3. `ON CONFLICT` arbiters

| arbiter | count | status | action |
|---|---|---|---|
| `(organization_id, …)` composite | 35 | **A** | PROMPT 3 tenant-composite swaps — correct |
| `(id)` | 7 | **A/E** | `orders`/`positions`/`trades` use globally-unique text ids; attribution is bound in the INSERT and the write scope is checked first (`TenantMismatch`) — cross-tenant re-attribution is impossible and PROMPT 3's PG suites prove isolation; the remaining sites are infra tables (`schema_migrations`-class) — intentional global |
| `(signature)` | 2 | **A/E** | on-chain signature is a global identity; the tenant repo binds the acting org, and a foreign re-record returns `Ok(false)` (documented + tested in PROMPT 3 and this prompt's PG suite) |
| `(event_id)` | 1 | **E** | `core/src/db/accounting.rs` deployment-global hash-chained audit ledger — intentional (single-operator accounting plane) |
| `(kind,id)`, `(worker_id)`, `(worker)`, `(role)`, `(flag)`, `(feed,…)`, `(key_hash)`, `(organization_id)`, `(path)` | 1–2 each | **E** | infra/registry tables (wallet registry, system flags, api keys, migrations bookkeeping) — intentional global, no tenant truth |
| `…` / doc-comment mentions | 4 | documentation | no SQL |

## 4. Provider / protocol / billing markers

| finding | location | status | action |
|---|---|---|---|
| `UnsupportedProvider` ×10 | `core/src/billing/provider.rs:182,198,214`; `core/src/custody/provider.rs:26,42,178,203,226,337`; `core/src/custody/resolve.rs:201` | **B** | the provider-neutral custody contract already fails closed with explicit `UnsupportedProvider` for Vault/KMS/HSM (Option-B posture at core level). §F adds the server-side registry/health/sign-request/sign-response/audit + tests, reusing this contract — no fake success anywhere |
| `NOT_IMPLEMENTED` ×8 | `server/src/saas/{billing_webhook.rs:362, checkout.rs:163, custody.rs:784, payment_webhooks.rs:54, provider.rs:127,128,486}` | **A** | correct fail-closed 501 responses for unconfigured providers — preserved, not "fixed" |
| `billing_status` synthetic render | `server/src/saas/billing_status.rs:98–124` — synthesizes plan `"starter"` fallback, `subscription_status: "active"`, zero usage, `dunning Current` | **D** | §G replaces the authoritative path: real DB subscription/plan/usage; the synthetic render is removed from the production path (rule 4/5 violation today) |
| `usage_limits` ×5 / `payment_webhooks` ×2 | `saas-sdk/src/commercial.rs`, `server/src/saas/mod.rs`, `saas/billing_webhook.rs` | **B/F** | webhook verification + idempotency exist; usage aggregation is not DB-backed → §G files 41–47 |
| `OrderV2` ×12 | `module-polymarket/src/eip712.rs:141` (struct), `orders.rs` (V2 order params/signing) | **B** | V2-only confirmed (audit P0 #5). §D keeps V2 where valid and adds V3/async |
| `tradeIDs` ×0 | — | **C** | async execution response parsing absent → §D files 21–22 |
| `position_id` ×363 | accounting/orders/positions (Solana position identity) | **E** | distinct concept from Polymarket `positionID`; no action; the Polymarket position-backed path uses its own `position_id` field on V3 orders (§D) |

## 5. Package / docs / IP markers

| finding | location | status | action |
|---|---|---|---|
| `buyer-release/source` vs canonical | 27 diffs (this prompt's sniper/copy/solana-kit/server changes not mirrored) | **D** | audit P0 #1 confirmed live. §J files 66–67 rebuild + verify parity at the END of this prompt (rebuilding now would go stale again) |
| `release-manifest` references | 59 files; counts refreshed this prompt (520/34/101) — verified by `release_manifest_counts_and_version_are_current` | **A** | `scripts/update-current-audit.sh` still missing → §J file 68 |
| `Copyright (c) 2026` | 2 files | **H** | §K file 75 (IP handover checklist) |
| `"UNKNOWN"` ×1 (non-test) | `server/src/ops/config_diff.rs:22` — `Self::Unknown => "UNKNOWN"` classification label | **A** | machine-readable diff classification label, production-safe |
| `placeholder` ×12 (non-test) | `server/src/staking/deployment_contract.rs:32–33` (detects + redacts the `3vEEMM…` placeholder program id); `saas/billing_status.rs:107,109` (synthetic billing — §D above); `billing/paddle_adapter.rs:166,529` (comments describing why fabricated placeholders are REJECTED); `dashboard.rs:89` (HTML input `placeholder` attribute); `ops/audit_attestation.rs:19` (comment) | **B/D/A** | staking: detection exists, deterministic validation + explicit status doc missing → §K file 74; billing_status → §G; the rest are production-safe comments/attributes |

## 6. Additional baseline facts recorded for the result doc

* Polymarket engine: 20 source files + 9 test files; `clob.rs` has `POST /order` with `OrderResponse` (status `matched/live/unmatched`, `makingAmount/takingAmount`) but NO `tradeIDs`/async-hash field; `eip712.rs` has full V2 EIP-712 (`Order` type with `builder` field, `signatureType`, salt/timestamp) — the V3/async surface is genuinely absent (audit P0 #5 accurate).
* Custody: `server/src/custody/` contains only `live_provider_contract.rs` + `live_provider_fixture.rs` (test fixture contract); the real provider registry/health/sign plumbing is absent → §F.
* Billing server-side: `server/src/billing/` has `stripe_adapter.rs`, `paddle_adapter.rs`, `provider_registry.rs` (BILLING providers), `live_provider_contract.rs/fixture.rs`; authoritative state model/usage/entitlement snapshot missing → §G.
* Customer API: `server/src/trading_data_plane/` already serves authenticated tenant-scoped orders/positions/executions/copy/polymarket routes (`/api/tenant/*`) — §H files 50–54 must EXTEND these (rule 35), not duplicate; missing: `customer_api` boundary, `bots` runtime controls, `tenant_authorization` chain module, isolation test file.
* Customer UI: `apps/control-plane/src/app/{billing,custody,settings}` exist; NO trading pages → §I files 58–65.
* Staking program id: placeholder `3vEEMM…` detected/redacted in `staking/deployment_contract.rs`; no deterministic validator or status doc → §K file 74.
* Phase-0 verdict on the P0 list: 1 confirmed live (buyer parity), 2 partially closed (module wiring for sniper/copy/solana DONE this prompt; polymarket/telegram/module-runtime open), 3 verified closed at repo layer (tenant repositories — but re-verified), 4 confirmed (custody = Option B), 5 confirmed (V2-only), 6 confirmed (no trading UI), 7 confirmed (`billing_status.rs` synthetic), 8 confirmed (scripts/docs stale), 9 confirmed (placeholder id), 10–12 documentation/evidence gaps → §K.
