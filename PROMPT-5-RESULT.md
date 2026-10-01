# PROMPT 5 RESULT — Polymarket V3 + Async + Custody + Billing + Customer SaaS + Buyer Release

Date: 2026-09-30 · Version 0.1.0 · Validation: full workspace 3306 passed / 0 failed / 14 ignored

---

## 1. PRE-CODING FORENSIC MATRIX

The authoritative spec (107 planned files, sections A–M) was mapped to the
existing codebase BEFORE coding. Full per-file mapping: `PROMPT-4-PROGRESS.md`
§ "PROMPT-5 SPEC MAPPING". Summary:

| Spec section | Files | Disposition |
| --- | --- | --- |
| A–D (1–34) Polymarket | 34 | Already existed as flat equivalents in `crates/module-polymarket` (orders/eip712/exchange_v3/position_orders/async_commit/trade_resolution/backfill/reconcile_async/tenant_context/tenant_executor + test suites). All 13 spec test cases covered by passing suites. Kept (spec's own no-duplicate rule); mapping documented. |
| E (35–40) core custody | 6 | Existed (`core/src/custody/{model,policy,resolve,provider,rotation,credentials,health}.rs`). |
| F (41–47) custody boundary | 7 | Existed (`server/src/custody/*`, `tests/custody_boundary.rs`). |
| G (48–52) Vault | 5 | **MISSING → CREATED** `server/src/custody/vault/{client,config,signer,health,mod}.rs`. |
| H (53–57) KMS | 5 | **MISSING → CREATED** `server/src/custody/kms/{client,config,signer,health,mod}.rs`. |
| I (58–69) billing | 12 | Existed as `saas/{billing_view,billing,billing_webhook,checkout,invoices,usage_limits,commercial_state,billing_reconciliation}.rs` + `billing/{stripe,paddle}_adapter.rs` + `tests/billing_{integration,authoritative_state}.rs` (provider-ADAPTER dir ≠ section I; authoritative state is the saas path). |
| J (70–73, 78–80) trading API | 7 | Existed as `trading_data_plane/{orders,positions,executions,copy,polymarket,recovery,service,authorization_chain,bots}.rs` + tests. |
| J (74–77) module controls/status | 4 | **MISSING → CREATED** `trading_data_plane/{sniper,telegram,module_controls}.rs` + status/controls handlers added to `copy.rs`/`polymarket.rs` (extend, not duplicate — the spec files' customer_api/ dir maps onto the existing plane per the no-redundancy rule). |
| K (81–94) customer UI | 14 | **MISSING → CREATED** 8 pages + 5 components + `lib/customer-trading-api.ts`. |
| L (95–100) buyer parity | 6 | **MISSING → CREATED** 4 scripts + `tests/release/buyer_source_parity.sh` + `docs/CURRENT-BUYER-STATE.md`. |
| M (101–107) evidence docs | 7 | **MISSING → CREATED** all 7, plus the spec-required claim-rejection script `scripts/verify-marketing-claims.sh`. |

**P0 found during the audit and fixed first**: `saas/custody_rotation.rs`
fabricated a synthetic `CustodyProfileId` per rotation. Fixed (see §3).

**Found during the FINAL ACCEPTANCE audit (spec criteria A–T, after this
report first shipped) and fixed**:

* **Criterion I violation** — `saas/usage_limits.rs` `render()`
  synthesized usage numbers (hardcoded `42.0` monthly-orders / `2.0`
  members) and hardcoded a `"pro"` plan picked from the default catalogue.
  Fully rewritten: plan from the REAL subscription
  (`subscription_of` → `plan`), totals from REAL recorded usage events
  (`usage_total`, idempotent), no-subscription → honest
  `plan_code:"none"`/`plan_source:"none"` with usage-only rows
  (`limit:null`, `allows:false`) — matching the billing-view invariant
  "no subscription → no plan surfaced". Tests now exercise the production
  path (`render_from_store`) incl. idempotent-dedup 12.0 sum,
  cross-tenant isolation, suspended tenant, no-secret payload (7 tests).
* **Criterion P** — ~13 docs still said Vault/KMS "not implemented",
  contradicting the real adapters built this prompt. All swept to the
  truth (adapters real + unit-tested; HSM fail-closed unimplemented;
  GAP-002 narrows to live validation for Vault/KMS). `AUDIT.md` §13.8
  remediation note appended (audit findings preserved, dated).
* **Flaky test fixed** — `solana-kit` `tenant_signing_context` asserted a
  base58 pubkey length of exactly 44; ~0.4% of random keys encode to 43
  (leading zero byte). Now asserts the address round-trips as a real
  `Pubkey` (43 or 44 chars). This was the one failure in the first
  post-fix full-workspace run.

See §13 for the full A–T table.

## 2. POLYMARKET V3

All statements are the state BEFORE this prompt's work (A–D existed and
passed) plus this prompt's verification:

- **V2**: `eip712.rs` — domain `Polymarket CTF Exchange`, version `"2"`,
  full `Order` typed struct (incl. `metadata`/`builder`), keccak EIP-712
  hashing, unit-tested. V2 regression suite passes.
- **V3**: `exchange_v3.rs` + `position_orders.rs` — explicit V3
  position-order support; `token_id` XOR `position_id` enforced
  (both-set is a typed error); invalid V3 positions rejected
  client-side (tested).
- **position_id semantics**: preserved exactly across V2/V3 — no
  transformation, validated as the order asset selector.
- **Async** (`async_commit.rs`): order accepted; transaction hash absent
  (preserved as "nothing yet"); trade IDs present; trade IDs absent;
  later transaction resolution; final success/failure — all separate
  states, all tested. Async is NEVER converted to immediate
  `confirmed`.
- **No duplicate on retry**: idempotency keys collapse duplicates
  (tested).
- **Tenant context**: `tenant_context.rs` → `tenant_executor.rs` → async
  polling → trade resolution → reconciliation → persistence, attached at
  submission, never re-derived from responses (tested in
  `tests/async_commit_pipeline.rs` + `tenant_execution.rs`).
- **Backfill / reconciliation**: `backfill.rs`, `reconcile.rs`,
  `reconcile_async.rs` — compatible with the existing reconciliation
  model (fills matched by venue ids, tenant-scoped persistence).
- **Marker search**: `OrderV2` = the V2 EIP-712 struct (correct, V2
  stays supported; no V3→V2 downgrade path exists — separate structs);
  `tradeIDs` = the camelCase wire field, handled with aliases in
  `clob.rs` + `async_commit.rs` (tested).

## 3. CUSTODY

| Provider | State | Exact reason / dependency |
| --- | --- | --- |
| local | Adapter present; single-operator deployments sign via the existing solana module wallet path; the multi-tenant boundary refuses with the dependency named | boundary-local signing for multi-tenant custody (deliberate: the wallet path stays authoritative) |
| **Vault** | **IMPLEMENTED (this prompt)** — real transit-engine REST client (reqwest, no SDK needed): `sys/health`, `token/lookup-self`, `transit/keys/{key}` (ed25519 + public key), `transit/sign/{key}`; strict `vault:vN:<base64-64-byte>` signature parsing; token in a redacted wrapper, never logged; per-signer key via `provider_ref` or `VAULT_TRANSIT_KEY`; resolution verifies provider type → status → key ref → key type → **public-key match** (new core error `PubkeyMismatch`) | Evidence level: UNIT_TEST (wire construction, envelope parsing, fail-closed paths, unreachable-port refusals). NO live Vault round-trip — the honest limitation, stated in `docs/CUSTODY-STATUS-2026.md`. |
| **KMS** | **IMPLEMENTED (this prompt)** — real SigV4-signed AWS KMS client (reqwest + hmac + sha2, no AWS SDK needed for the env-credential-chain shape): `TrentService.GetPublicKey` (strict Ed25519 SPKI parsing) + `TrentService.Sign` with `EDDSA_SHA_512` (AWS KMS supports Ed25519 since Nov 2025); credentials from the standard AWS env chain, read at request time, never rendered | Evidence level: UNIT_TEST (SigV4 signing key verified against the AWS-documented test vector `c4afb1cc…a4b9`; deterministic signed-request construction; fail-closed paths). NO live KMS round-trip. |
| HSM | UNAVAILABLE — fail-closed refusal naming the exact dependency | `PKCS#11 module with HSM_SLOT and an HSM_PIN reference (pkcs11 sign integration)` |

**P0 fix (custody rotation)**: `RotationRequest` now REQUIRES `profile_id`;
`create()` parses it (400 on malformed), resolves the REAL profile through
the tenant-scoped store (404 for missing/foreign — no oracle), refuses
non-Active profiles (409 `profile_not_active`), and verifies BOTH signers
exist, are owned by the tenant, and belong to the profile (404/409
otherwise). The synthetic `CustodyProfileId::new()` lines are deleted. The
UI's rotation form was rewritten to select real profile/signers (the old
form invented a random UUID as the new signer — impossible against the
honest backend).

**Health semantics upgraded**: with real adapters, an answered Vault/KMS
probe is genuinely `ready` (was `configured_unsupported` under OPTION-B);
unreachable = `unreachable`; missing credentials/references =
`configured_unsupported`/`missing_references` with the exact dependency.
Integration-tested in `custody/health.rs` + `provider_registry.rs`.

## 4. BILLING

Unchanged by this prompt (section I equivalents existed and passed):
subscription/payment/invoice transitions are deterministic
(`saas/billing.rs` + core state machine); provider events persist with
idempotency — webhook replays collapse (`billing_webhook.rs`,
`payment_webhooks.rs`); payment application is transactional; usage
metering + entitlement enforcement gate the whole customer API
(`usage_limits.rs`, `bot-core::billing`). Stripe/Paddle adapters implement
the real API surfaces behind one live-provider contract. PG-backed suites:
`billing_integration` (9), `billing_authoritative_state` (7),
`live_billing_contract` (2+3 ignored) — all pass. **No LIVE provider
round-trip exists** (stated in `docs/BILLING-STATUS-2026.md`).

## 5. CUSTOMER API

Chain (unchanged, now with two more module families):

```
authenticate (session/tenant API key)
→ tenant (organization from the credential ONLY — x-organization is a re-verified hint)
→ runtime (trading data plane attached, else 503 trading_data_plane_unavailable)
→ lifecycle (suspended/closed → 403 tenant_lifecycle_blocked)
→ entitlement (family feature(s) — sniper/copy/polymarket; telegram = control-plane carve-out, documented in one place)
→ repository (tenant-scoped predicates; cross-tenant = not-found)
→ response (stable error codes, no secrets)
```

**New in this prompt**: `GET /api/tenant/{sniper,copy,polymarket}/status`
+ `POST …/controls` (enable/disable, BotStart/BotStop permissions,
tenant-scoped override store, runtime lifecycle stays runtime-owned and
fenced) and `GET /api/tenant/telegram/status` +
`PUT/DELETE /api/tenant/telegram/binding` (tenant-scoped chat-id binding;
honest note that per-tenant routing is a deployment-side step). The UI
client (`lib/customer-trading-api.ts`) refuses any path outside
`/api/tenant/*` — customer pages cannot reach operator-global endpoints.

## 6. CUSTOMER UI

Created (all "use client", typed, no synthetic numbers; every surface
renders loading/empty/error/suspended/entitlement-denied/module-disabled/
stale-runtime/plane-unavailable/custody-unavailable):

- `src/app/trading/page.tsx` — dashboard (PnL + RuntimeCard + ModuleCards)
- `src/app/trading/orders/page.tsx` — server-paginated orders + execution drill-down + cancel
- `src/app/trading/positions/page.tsx` — positions + PnL card
- `src/app/trading/executions/page.tsx` — execution lifecycle over a window
- `src/app/trading/sniper/page.tsx`, `…/copy/page.tsx` (leaders + controls), `…/polymarket/page.tsx` (mirror book + fills + controls)
- `src/app/trading/telegram/page.tsx` — status + binding management
- `src/components/trading/{RuntimeCard,OrderTable,PositionTable,PnlCard,ModuleCards}.tsx`
- `src/lib/customer-trading-api.ts` — the customer-only typed client
- Modified: `src/lib/api.ts` (PUT method), `src/lib/commercial.ts`
  (rotation now sends `profile_id`), `src/app/custody/page.tsx` (real
  rotation form).

Gates: `tsc --noEmit` clean · `eslint` 0 errors (warnings match the
pre-existing pattern class) · `next build` succeeds — all 12 routes
compile, including the 8 new trading routes.

## 7. BUYER PACKAGE

```
canonical (product tree, 828 files)
  → scripts/rebuild-buyer-release.sh  (mirror: copied=0 unchanged=828 after final run; exclusions identical to the compare's)
  → buyer-release/source              (byte-exact: 828/828 identical, 0 missing, 0 stale, 0 differs)
  → manifest                          (release-manifest.json regenerated by scripts/update-release-manifest.sh:
                                       product_files=828, rust_files=561 (crates/ convention), docs_files=109,
                                       migrations=34 high-water 0034, tree_digest; last_full_suite recorded honestly)
  → checksums                         (buyer-release/checksums/source.sha256 — every source file sha256; Cargo.lock.sha256)
  → verification                      (scripts/verify-release-integrity.sh: [1/4] parity OK, [2/4] manifest counts
                                       verified, [3/4] contamination OK — no secrets/keys/wallets/dumps/bulds/binaries,
                                       the leaked .cargo/bin/rustup toolchain binary was found and excluded,
                                       [4/4] version 0.1.0 consistent across VERSION/Cargo.toml/manifest/buyer)
```

Regression: `tests/release/buyer_source_parity.sh` PASSES — real-tree
parity holds AND the compare is proven to detect a planted tampered file
(a checker that always says OK would be worthless).

## 8. MARKETING

Registry: `docs/MARKETING-CLAIMS.md`; enforcement:
`scripts/verify-marketing-claims.sh` (exit 0 today).

- **SAFE CLAIMS** (each with evidence file + level): multi-tenant control
  plane with repository-layer isolation (INTEGRATION_TEST); copy-trading
  engine with recovery (UNIT_TEST); Polymarket V2 + explicit V3
  position-order support (UNIT_TEST); custody boundary with full audit
  trail (UNIT_TEST); authoritative billing state machine with webhook
  idempotency (INTEGRATION_TEST); fail-closed provider design (CODE);
  customer trading API with module controls (INTEGRATION_TEST); PG17 +
  34 forward-only migrations (INTEGRATION_TEST).
- **UNSUPPORTED CLAIMS** (rejected by the script unless line-tagged with
  evidence at the required level): "guaranteed", "profitable",
  "risk-free" (no level justifies); "under 1 second guaranteed"
  (needs LIVE_TEST); "fully audited", "fully isolated", "institutional
  SLA" (need EXTERNAL_AUDIT); "all Raydium", "latest Polymarket V3",
  "Vault/KMS/HSM included", "self-service billing" (need LIVE_TEST);
  "mainnet proven" (needs FUNDED_TEST).
- **EVIDENCE LEVEL**: CODE < UNIT_TEST < INTEGRATION_TEST < LIVE_TEST <
  FUNDED_TEST < EXTERNAL_AUDIT — no claim exceeds its level
  (`docs/LIVE-EVIDENCE-MATRIX.md` maps every capability to its strongest
  actual evidence).

## 9. VALIDATION

Exact commands and exact results (2026-09-30, rustup 1.98.1, PG 17.11,
`CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`):

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean (after one `cargo fmt --all` pass) |
| `cargo check --workspace --all-targets` | clean, 0 errors |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, 0 warnings/errors |
| `cargo test -p module-polymarket` | 136+7+7+8+4+5+11+10… all pass |
| `cargo test -p solana-kit` | 270+4+5+4+2+9+0 all pass |
| `cargo test -p sniper-suite` | **1774 passed, 0 failed, 12 ignored** |
| `cargo test -p bot-core` | 727 passed, 0 failed, 1 ignored (no PG) · with PG: +26 db_integration |
| `cargo test -p module-sniper` | 144 passed, 0 failed, 1 ignored |
| `cargo test -p module-copy` | 110 passed, 0 failed |
| `cargo test -p module-telegram` | 21 passed, 0 failed |
| `cargo test -p saas-sdk` | 32 passed, 0 failed |
| `POSTGRES_URL=… cargo test -p sniper-suite --test billing_integration … -- --test-threads=1` (9 PG-backed suites) | 50 passed, 0 failed, 6 ignored across the 9 suites |
| `cargo test --workspace` (default parallelism) | FAILED — bot-core `db_integration` audit-chain tests raced on the shared PG DB (3 failed); solana-kit `latency_bench` earlier failed at link stage with `ld: signal 7 bus error` (sandbox disk exhaustion: target/ 14G, 100% full) — both environmental |
| `cargo test --workspace -- --test-threads=1` (clean tenant_test DB) | **3306 passed, 0 failed, 14 ignored** — FULL WORKSPACE |
| **Post-acceptance-fix re-run** `cargo test --workspace -- --test-threads=1` (2026-09-30, after criterion I/P fixes + flaky-test fix; `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`) | **3312 passed, 0 failed, 14 ignored** across 104 suites (+7 new usage_limits tests, −1 old comment-only stub; includes the fixed base58-length test) |
| `cargo test -p sniper-suite --lib "saas::"` (post-fix) | 108 passed, 0 failed (includes the 7 rewritten usage_limits tests + custody/billing suites) |
| `cargo test -p sniper-suite --test batch6_transaction_readiness` (post-fix, after release rebuild) | 12 passed, 0 failed (docs count 109 unchanged — criterion P edits only, no files added/removed) |
| `tsc --noEmit` / `eslint src` / `next build` (apps/control-plane) | clean / 0 errors / 12 routes built |
| `scripts/verify-marketing-claims.sh` | OK |
| `scripts/rebuild-buyer-release.sh` | mirror verified 828/828 |
| `scripts/verify-release-integrity.sh` | ALL 4 CHECKS OK |
| `tests/release/buyer_source_parity.sh` | PASS (incl. planted-drift detection) |

**Post-code marker search — every match classified**:
`TODO`: 0 in our code (76 hits were all `node_modules` third-party). ·
`FIXME`: 0. · `NOT_IMPLEMENTED`: 8 — all honest HTTP 501 refusals for
unsupported providers/routes + their mappings/tests. · `placeholder`: all
classified — anti-placeholder GUARDS (staking refuses placeholder program
ids, tested), redaction docs, UI `<input placeholder=…>` attributes,
cross-references to the core OPTION-B adapters (2 stale "Batch 5
placeholder" doc-comments on real test files were REWRITTEN, and the
misleading "placeholder" comment in `payment_webhooks.rs` test helper was
reworded). · `stub`: core OPTION-B adapters (documented fail-closed
refusals) + live-contract test fixture docs (clearly NON-LIVE). ·
`UnsupportedProvider`: 21 — the typed fail-closed error + its handling.
· `starter`/`pro`: no plan-name claims **(CORRECTED 2026-09-30: the
original classification was WRONG — the markers pointed at real synthetic
values in `saas/usage_limits.rs` `render()`: hardcoded `42.0`/`2.0` usage
totals and a hardcoded `"pro"` plan from the default catalogue. The file
was fully rewritten to derive every number from the store; see §13
criterion I)**. · `42`/`2`: **CORRECTED likewise** — the earlier
"incidental hex-sha substrings" classification missed the code hit
`42.0`/`2.0` in `usage_limits.rs` (found by the final acceptance audit,
fixed; the docs-only sweep had reported the markers clean because the
values lived in code, not docs). · `manual`: no
manual-process claims in current docs. · `payment_state`: the
authoritative billing state field (correct usage). · `usage = 0`: 0
matches. · `OrderV2`: the V2 EIP-712 struct (V2 stays supported; no
downgrade). · `tradeIDs`: the async wire field, alias-parsed + tested. ·
`position_id`: V3 order asset selector, XOR-enforced with token_id. ·
`organization_id`: never client-chosen (chain derives it from the
credential; `x-organization` is a re-verified hint). ·
`TenantExecutionContext`: the execution authority model, attached across
the whole trading pipeline. · `operator`: operator-console vs
customer-plane separation (AppShell operator sections; customer pages
cannot reach operator endpoints). · `buyer-release/source`: parity
verified byte-exact.

## 10. REMAINING GAP

| Dimension | Gap | Why |
| --- | --- | --- |
| 5-module engineering | **~2%** | All modules implemented, tested, integrated. Remaining: per-tenant telegram alert ROUTING consumes the binding (API + store exist); module-controls store is process-local (DB migration is mechanical). |
| Enterprise SaaS gap | **~8%** | Customer API/UI/billing/custody complete and integration-tested. Remaining: SSO/SAML, org-level roles beyond the permission set, regional data residency, SLA/ops runbooks with on-call — none claimed. |
| Business/product gap | **~15%** | Product is deliverable and honest. Remaining: pricing/packaging finalization, live checkout enablement, marketing site, sales/acceptance process with a real buyer. |
| Evidence gap | **~30%** | Everything above UNIT_TEST/INTEGRATION_TEST: LIVE_TEST (Vault, KMS, Stripe/Paddle, Polymarket market data), FUNDED_TEST (mainnet), EXTERNAL_AUDIT (security). All absence is documented and enforced by the claims gate. |

## 11. FULL CODE

Every new/modified file (complete, no truncation) lives in the workspace
and the byte-verified buyer mirror:

**New — Rust (16)**: `crates/server/src/custody/vault/{mod,client,config,signer,health}.rs`,
`crates/server/src/custody/kms/{mod,client,config,signer,health}.rs`,
`crates/server/src/trading_data_plane/{module_controls,sniper,telegram}.rs`.

**New — TypeScript (14)**:
`apps/control-plane/src/app/trading/{page,orders/page,positions/page,executions/page,sniper/page,copy/page,polymarket/page,telegram/page}.tsx`,
`apps/control-plane/src/components/trading/{RuntimeCard,OrderTable,PositionTable,PnlCard,ModuleCards}.tsx`,
`apps/control-plane/src/lib/customer-trading-api.ts`.

**New — scripts/tests/docs (13)**:
`scripts/{compare-canonical-to-buyer-source,rebuild-buyer-release,update-release-manifest,verify-release-integrity,verify-marketing-claims}.sh`,
`tests/release/buyer_source_parity.sh`,
`docs/{MARKETING-CLAIMS,LIVE-EVIDENCE-MATRIX,POLYMARKET-COMPATIBILITY-2026,CUSTODY-STATUS-2026,BILLING-STATUS-2026,CUSTOMER-SaaS-STATUS-2026,BUYER-HANDOVER-STATUS-2026,CURRENT-BUYER-STATE}.md`.

**Modified (11)**: `crates/core/src/custody/provider.rs` (+`PubkeyMismatch`),
`crates/server/src/custody/{mod,provider_registry,health,sign_boundary}.rs`
(wiring + honest health semantics), `crates/server/src/saas/{custody,custody_rotation}.rs`
(P0), `crates/server/src/trading_data_plane/{mod,copy,polymarket,authorization_chain}.rs`,
`crates/server/Cargo.toml` (+base64), `apps/control-plane/src/{lib/api,lib/commercial,app/custody/page}.tsx`,
`crates/server/tests/batch6_transaction_readiness.rs` (doc-count pin 101→109),
`release-manifest.json`, 4 test-file doc comments corrected.

**Modified in the final acceptance-audit pass (§13)**: `crates/server/src/saas/usage_limits.rs`
(**full rewrite** — criterion I: real subscription-derived plan + real store
usage totals + honest no-subscription rendering, 7 production-path tests),
`crates/solana-kit/src/tenant_signing_context.rs` (flaky base58-length
assertion → round-trip assertion), `crates/server/src/saas/custody.rs`
(resolve endpoint stale comment + reason string corrected, behavior
unchanged fail-closed), and the criterion P doc sweep: `docs/{SECURITY,
SELLER-FACT-SHEET,SELLING-LISTING-SOURCE,BUYER-TRUTH-REGISTER,
BUYER-VERIFICATION-SCRIPT,FINAL-BUYER-STATUS,BUYER-EVIDENCE-PACK,
EXTERNAL-VALIDATION-RUNBOOK,FINAL-BUYER-GAP-LEDGER,
FINAL-EXTERNAL-VALIDATION-MATRIX,SECURITY-CONTROLS-MATRIX,
TECHNICAL-DIFFERENTIATORS}.md` + `AUDIT.md` §13.8 remediation note
(original 2026-09-29 findings preserved verbatim). Docs count unchanged
(109).

**Second documentation pass (same date, criterion-P completion):**
`docs/SAAS-PRODUCT.md` (billing-reality, console, and limitations
sections rewritten to the post-PROMPT-5 truth), `CHANGELOG.md` (the
PROMPT 2–5 arc entry added to [Unreleased]), `AUDIT-REMEDIATION-2026-09-29.md`
(§1 verbatim-install claim corrected for the appended notes; §8
post-round status table), `AUDIT.md` (additional dated remediation notes
§14.6 billing, §15.5 customer UI, §16 Polymarket V3 — original findings
still verbatim).

## 12. NO-OMISSION DECLARATION

- **No code skipped.** Every file listed in §11 is complete and compiles
  under `cargo check --workspace --all-targets` / `tsc --noEmit` /
  `next build`.
- **No placeholders.** Zero `TODO`/`FIXME` in our source; all
  "placeholder" marker matches are anti-placeholder guards, UI input
  attributes, or documentation of fail-closed refusals (two stale
  doc-comments were rewritten).
- **No fake providers.** Vault and KMS are real wire-protocol clients;
  every signature comes from a real provider response or the path fails
  closed with a typed error. HSM refuses with its exact dependency. No
  LIVE_TEST is claimed.
- **No fake billing.** Billing state is the deterministic authoritative
  machine; webhook events are idempotent; the live-provider fixture is
  test infrastructure only, clearly labeled.
- **No synthetic usage.** After the §13 criterion-I fix, every
  usage/limits number on the customer surface is derived from the store
  (subscription → plan; recorded idempotent usage events → totals); an
  organization without a subscription sees `plan_code:"none"` and its
  real usage — never a demo tier or demo numbers.
- **No truncated files.** The buyer mirror is byte-verified 828/828
  identical to the canonical product (`verify-release-integrity.sh` all
  green), and the source checksums are regenerated per rebuild.

## 13. FINAL ACCEPTANCE CRITERIA (spec A–T) — AUDITED 2026-09-30

Audited against the shipped tree AFTER the fixes above. "Evidence" cites
the verifying artifact or command; every command was re-run this date.

| # | Criterion | Verdict | Evidence |
| --- | --- | --- | --- |
| A | Polymarket V3 on the real path | **PASS** | `exchange_v3.rs` is the EIP-712/order path; V3 suite green in the 3312-test run |
| B | Async trade resolution | **PASS** | async_commit/trade_resolution suites; `tradeIDs` alias-parsing tests |
| C | V2 stays working | **PASS** | V2 suites green; `OrderV2` marker classified (V2 EIP-712 struct, no downgrade) |
| D | Tenant context through the pipeline | **PASS** | `TenantExecutionContext` + `tenant_signing_context` adapter (incl. the fixed round-trip test) |
| E | Custody domain fail-closed | **PASS** | `core::custody` policy/resolve/health; HSM refusal names its dependency |
| F | Custody boundary guard-ordered | **PASS** | `sign_boundary.rs` (policy → health-gated resolution → provider sign; audited) |
| G | Vault transit adapter real | **PASS** | `server/src/custody/vault/` — real REST client, unit-tested wire, redacted token |
| H | AWS KMS adapter real | **PASS** | `server/src/custody/kms/` — SigV4 verified vs the AWS test vector, `EDDSA_SHA_512` |
| I | **Usage is real DB-derived data** | **PASS (after fix)** | was VIOLATED (synthetic 42.0/2.0 + hardcoded "pro"); `usage_limits.rs` rewritten — `subscription_of`→`plan`, `usage_total` sums recorded idempotent events, no-sub → `plan_code:"none"`; 7 production-path tests green |
| J | Customer trading API tenant-safe | **PASS** | `/api/tenant/*` chain + repository predicates; cross-tenant = not-found; route+repo tests green |
| K | Customer UI uses customer API only | **PASS** | `customer-trading-api.ts` path allow-list; tsc/eslint/next build clean |
| L | Buyer package parity | **PASS** | rebuild 829/829 mirror, integrity 4/4, parity regression (planted drift detected), claims gate OK — re-run post-fix |
| M | Marketing claims evidence-backed | **PASS** | `verify-marketing-claims.sh` OK (re-run post-fix) |
| N | Sniper/copy suites | **PASS** | 144 + 110 tests in the 3312 run |
| O | Billing authoritative + idempotent | **PASS** | authoritative-state + integration suites; provider_events idempotency |
| P | **Docs don't contradict code** | **PASS (after fix)** | was VIOLATED (~13 docs said Vault/KMS "not implemented" vs the real adapters); swept: SECURITY.md, SELLER-FACT-SHEET, SELLING-LISTING-SOURCE, BUYER-TRUTH-REGISTER (6 rows + dated update), BUYER-VERIFICATION-SCRIPT, FINAL-BUYER-STATUS, BUYER-EVIDENCE-PACK, EXTERNAL-VALIDATION-RUNBOOK (3 sites), FINAL-BUYER-GAP-LEDGER (row + dated note), FINAL-EXTERNAL-VALIDATION-MATRIX, SECURITY-CONTROLS-MATRIX, TECHNICAL-DIFFERENTIATORS, `AUDIT.md` §13.8 note; `saas/custody.rs` stale comment+reason corrected; docs count stays 109. **Second sweep (same date):** `SAAS-PRODUCT.md` rewritten (billing/checkout/invoices/customer-console/tenant-data-plane claims were pre-PROMPT-5), `CHANGELOG.md` [Unreleased] gained the PROMPT 2–5 arc entry, `AUDIT-REMEDIATION-2026-09-29.md` §1 verbatim-install claim corrected + §8 post-round status table, `AUDIT.md` gained dated remediation notes §14.6/§15.5/§16 (billing synthetic, customer UI, Polymarket V3 — findings preserved verbatim) |
| Q | No placeholders in touched paths | **PASS** | marker sweep re-verified; 0 TODO/FIXME in touched files |
| R | Entitlements enforced | **PASS** | entitlement checks in `authorization_chain` + middleware chain tests |
| S | Reconciliation tenant-aware | **PASS** | tenant-scoped reconciliation queue + reconciliation suite |
| T | Full contents of new/modified files shipped | **PASS** | every file complete in the workspace AND byte-verified in the buyer mirror (829/829, checksums per rebuild); §11 indexes them |

**Result: A–T all PASS** (I and P after fixes found by this audit; the
audit itself is why they were found).

**Post-fix full gate (2026-09-30):** `cargo fmt --all -- --check` clean ·
`cargo clippy --workspace --all-targets -- -D warnings` 0 warnings ·
`cargo test --workspace -- --test-threads=1` **3312 passed / 0 failed /
14 ignored** · `batch6` 12/12 (docs 109) · release rebuild + integrity
4/4 + parity regression + claims gate all green.

**Honest residual (unchanged):** no LIVE_TEST for Vault/KMS/Stripe/Paddle
(`LIVE_CUSTODY=1`/`LIVE_BILLING=1` buyer-side), no FUNDED_TEST, no
external audit, HSM unimplemented (fail-closed). All documented in
`docs/CUSTODY-STATUS-2026.md`, `docs/BUYER-TRUTH-REGISTER.md`,
`docs/FINAL-BUYER-GAP-LEDGER.md`.
