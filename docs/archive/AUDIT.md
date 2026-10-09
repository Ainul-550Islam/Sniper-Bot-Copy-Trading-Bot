> **Seller's internal engineering log — not an independent or external audit.** Statuses in this file are the seller's own and were not verified by a third party.

# SNIPER-SUITE — CURRENT $20K–$60K BUYER AUDIT
## Line-level / file-level / business / competitor / marketing re-audit

**Audit date:** 2026-09-29  
**Source of truth:** uploaded workspace ZIP `workspace-01a0e861-8506-7c68-b1ae-32d4e99647ab.zip`  
**Canonical tree audited:** `Sniper-Bot-Copy-Trading-Bot/`  
**Audit type:** static source + schema + documentation + release-package + business/product comparison  
**Runtime limitation:** this audit environment does not have `cargo`; therefore no fresh Rust build/test result from this environment is represented as independently executed. Historical/project-provided validation is clearly labeled as seller-reported or repository-reported.

---

# 1. EXECUTIVE VERDICT

## 1.1 Current estimated gap

| Scope | Estimated completeness | Estimated gap | Interpretation |
|---|---:|---:|---|
| Exact 5-module engineering scope | ~88–92% | ~8–12% | Core Sniper/Copy/Polymarket/Stake/Telegram functionality is substantial. |
| Single-operator commercial software/IP asset | ~70–80% | ~20–30% | Strong code, but deployment proof, live evidence, staking identity/audit, custody and documentation integrity still matter. |
| True enterprise multi-tenant SaaS | ~52–60% | ~40–48% | Tenant foundation exists, but the actual money-moving modules and production DB repositories are not yet fully tenant-wired. |
| Broad terminal/product parity vs major competitors | ~35–50% | ~50–65% | Competitors have broader analytics, customer UX, multi-chain/multi-exchange and discovery features. |

These percentages are engineering/product completeness estimates, not market appraisals or guaranteed sale prices.

## 1.2 Most important conclusion

The project is **not a fake/scaffold codebase**. It contains a substantial Rust trading infrastructure core, database migrations, HA/distributed execution, risk controls, Sniper/Copy/Polymarket engines, staking program, Telegram control, SaaS identity/runtime foundations and extensive documentation.

However, the current repository should **not** be marketed as a fully finished `$20k–$60k multi-tenant SaaS platform**. The strongest current buyer story is:

> **Advanced specialized Solana + Polymarket trading infrastructure/IP with a substantial Rust execution core, SaaS control-plane foundation, risk/recovery/HA infrastructure, and a clearly documented production-hardening backlog.**

A stronger `$40k–$60k` commercial claim requires closing the specific P0/P1 gaps listed in this report and regenerating a release package whose documentation, manifest and source mirror exactly match the current tree.

---

# 2. AUDIT METHODOLOGY

The audit was performed against the uploaded ZIP and included:

1. canonical source-tree inventory;
2. Rust file and LOC measurement;
3. migration inventory;
4. module-by-module inspection;
5. production DB repository SQL pattern scan;
6. tenant/security/custody/billing code inspection;
7. Polymarket protocol compatibility inspection;
8. staking program/account-validation inspection;
9. Telegram/control-plane inspection;
10. documentation consistency inspection;
11. buyer-release/source parity inspection;
12. placeholder/synthetic/stub scan;
13. business/product capability comparison against current public documentation for Solana trading and Polymarket software.

The audit distinguishes:

- **PRESENT** — real implementation found;
- **PARTIAL** — implementation exists but not complete for stated commercial claim;
- **MISSING** — required capability absent;
- **STALE** — documentation/package does not represent the current code;
- **UNVERIFIED** — code exists but required live/integration evidence is not established in the current environment.

---

# 3. CURRENT TREE MEASUREMENT

Measured from the canonical root in the uploaded workspace:

- **442 Rust files** under `crates/`, `programs/`, `apps/`;
- **183,640 Rust LOC**;
- **25 SQL migrations** (`0001` through `0025`);
- **101 Markdown documents** under `docs/`;
- **5 Next.js App Router page files** under `apps/control-plane/src/app/`;
- **1,419 non-hidden files** in the extracted workspace after excluding `.git`, `target`, and `node_modules` from relevant source checks.

Rust distribution:

| Component | Rust files | Approx. LOC |
|---|---:|---:|
| `crates/core` | 128 | 58,389 |
| `crates/module-copy` | 27 | 12,572 |
| `crates/module-polymarket` | 30 | 16,199 |
| `crates/module-sniper` | 19 | 13,027 |
| `crates/module-telegram` | 4 | 1,486 |
| `crates/saas-sdk` | 8 | 1,599 |
| `crates/server` | 197 | 52,437 |
| `crates/solana-kit` | 23 | 22,393 |
| `programs/staking-suite` | 6 | 5,538 |
| **Total** | **442** | **183,640** |

This is materially larger than the old numbers still repeated in historical documents.

---

# 4. RELEASE-PACKAGE INTEGRITY — REAL CURRENT PROBLEM

## 4.1 `verify-buyer-package.sh` currently FAILS

The current repository script was executed from the audited tree.

Observed result:

- required release files: OK;
- version consistency: OK;
- migration 0024: OK;
- checksums: OK;
- source-tree digest: OK;
- package evidence: present;
- **`DIFFERS release-manifest.json`**;
- **`DIFFERS PROMPT-2-RESULT.md`**;
- **23 current canonical tenant files are missing from `buyer-release/source`**;
- **15 shared tenant files differ in content**;
- final status: **FAIL**.

Representative missing files include:

- `crates/core/tests/tenant_scope_isolation.rs`
- `crates/core/tests/tenant_idempotency_isolation.rs`
- `crates/server/src/tenant/context.rs`
- `crates/server/src/tenant/context_guard.rs`
- `crates/server/src/tenant/context_resolver.rs`
- `crates/server/src/tenant/runtime_cache.rs`
- `crates/server/src/tenant/runtime_context.rs`
- `crates/server/src/tenant/signer_guard.rs`
- `crates/server/src/tenant/wallet_guard.rs`
- tenant background context/guard/identity files;
- tenant observability fields/redaction/audit-context files;
- tenant stream subscription-scope file;
- several tenant integration tests.

## 4.2 `verify-delivery.sh` is a different check

The generic delivery hygiene script passes its current checks, but it does **not** prove that the buyer-release source mirror is byte/content identical to the canonical current tree.

Therefore:

**`verify-delivery.sh PASS` does not override `verify-buyer-package.sh FAIL`.**

## 4.3 Buyer impact

This is a serious commercial credibility issue because the buyer may receive:

- current canonical source;
- stale/partial buyer-release source;
- stale manifest numbers;
- historical audit claims that no longer describe the current tree.

### Priority

**P0 — regenerate buyer-release from the canonical current tree, regenerate manifest counts/hashes, rerun all package checks, and distribute only the resulting bundle.**

---

# 5. DOCUMENTATION CONSISTENCY — REAL GAP

## 5.1 Root `AUDIT.md` is historically rich but currently internally inconsistent

The opening sections still describe very old state such as:

- 69 source files;
- 54 Rust files;
- ~27,119 LOC;
- no integration tests;
- no DB;
- no Redis;
- no Geyser.

Later sections document substantial remediation and newer counts.

The result is a document that contains both historical findings and current-state claims without a single current-state headline that definitively overrides the historical opening.

## 5.2 `AUDIT.md` historical commercial value sections are stale

The document still contains an old valuation section with:

- current codebase value ≈ $10k–$22k;
- $20k target after minimum hardening;
- $40k after hardening;
- $60k after enterprise productionisation.

That section was written for a much smaller pre-SaaS tree and is not a valid standalone current valuation statement for the 2026-09-29 tree.

It should be moved into a dated historical appendix.

## 5.3 `FINAL-BUYER-STATUS.md` is stale

It still advertises older snapshots:

- 21 migrations in one section;
- 521/537 historical test totals;
- frozen tree counts such as 146 files and 77,980 lines;
- source parity claims tied to the older freeze.

The current canonical tree has 25 migrations and 442 Rust files.

## 5.4 `SELLER-FACT-SHEET.md` is also stale in multiple fields

The fact sheet still contains:

- a 7-crate/old tree description;
- 21 migration count;
- frozen 146-file / 77,980-line measurement;
- old single-tenant limitations;
- old test counts.

Some limitations are still substantively true, but the snapshot numbers must be dated or replaced.

### Priority

**P0 documentation synchronization.**

---

# 6. MODULE 1 — SNIPER BOT

## 6.1 Present

`crates/module-sniper/src/detect.rs`

Current detection architecture includes:

- PumpPortal feed;
- Solana `logsSubscribe` paths;
- Geyser `transactionSubscribe` path;
- parsing and event normalization;
- fallback behavior.

`crates/module-sniper/src/pipeline.rs:452–464`

Current entry routes include:

- Pump.fun bonding curve;
- PumpSwap direct;
- Raydium AMM v4 direct;
- Jupiter.

`crates/module-sniper/src/entry.rs:243–340`

The pipeline performs staged:

- shape/route validation;
- kill/enabled checks;
- symbol gate;
- dedup;
- screening;
- market/risk validation;
- distributed ownership claim.

`entry.rs` also contains configured priority-fee/Jito handling around the later execution stages.

## 6.2 Important correction

Previous older audit language saying "Geyser missing" is now stale.

The current code DOES have Geyser-related detection logic.

## 6.3 Current real gap

The README claim at `README.md:9` says:

> Detects new pump.fun launches and buys within ~1s

That must be interpreted as a **target/benchmark**, not a landing guarantee.

Solana documentation states priority fees increase the likelihood that a leader schedules a transaction ahead of competitors; they do not guarantee inclusion. Solana production guidance also emphasizes fresh blockhashes, priority fees, compute-unit limits and production RPC infrastructure. Jito documents fast low-latency submission and 50ms auctions, while explicitly requiring status tracking because submission does not itself prove landing.

Therefore:

**SAFE marketing:**

> Low-latency pump.fun sniper designed for sub-second execution targets, with Geyser/PumpPortal ingestion, priority-fee support and optional Jito submission.

**UNSAFE marketing:**

> Guaranteed buy within 1 second.

## 6.4 Raydium breadth gap

Current `EntryRoute` explicitly implements direct Raydium AMM v4, not every current Raydium product.

Current Raydium documentation covers AMM v4, CPMM, CLMM, Farm/Staking, LaunchLab and Perps.

So:

- "Raydium AMM v4 support" = PRESENT;
- "all Raydium support" = FALSE;
- "Raydium + Jupiter routing" = PRESENT.

## 6.5 Sniper assessment

**Core module completeness: ~90–95%.**

Remaining commercial gap:

- landing-rate evidence;
- real funded validation;
- broader Raydium direct support if desired;
- production RPC/region optimization;
- customer-facing latency/landing dashboards.

---

# 7. MODULE 2 — COPY TRADING BOT

## 7.1 Present

Current source includes feed/replay infrastructure and tracked wallet copying.

The current architecture uses multiple detection/feed modes and keeps copy state with sizing/staleness/exit handling.

The current project is therefore NOT missing the core concept:

> tracked wallet buys → follower-side copy action.

## 7.2 Competitor business gap

Current Trojan documents describe copy trading with customizable buy/sell behavior, execution wallet settings, priority fee/bribe/slippage/MEV settings and risk filters; Trojan also exposes Wallet Analyzer, Token Audits, Real-Time Alerts, historical data and watchlists.

Current GMGN documentation describes wallet copying, follow tracking, real-time smart-money alerts, copy settings and automated buy/sell/TP/SL behavior. GMGN also documents Wallet Radar that can classify wallets by purchase count, profit and earliest purchase.

Your core copy engine is therefore strong but your PRODUCT SURFACE is smaller.

## 7.3 Missing business/product capabilities

- wallet performance analytics UI;
- smart-money leaderboard;
- wallet radar/discovery;
- wallet historical performance page;
- copy-task management UI;
- richer copy filters;
- customer notifications/alerts dashboard;
- polished copy performance analytics;
- mobile trading interface.

## 7.4 Copy assessment

**Core engine completeness: ~90–95%.**

**Product-surface gap vs leading Solana terminals: ~25–40%.**

---

# 8. MODULE 3 — POLYMARKET BOT

## 8.1 Present

Current `module-polymarket` includes:

- Gamma;
- CLOB REST;
- CLOB WebSocket;
- L1 auth;
- L2 HMAC authentication;
- EIP-712 V2 order signing;
- order lifecycle/recovery/reconciliation;
- CTF-related local state.

## 8.2 Current source is explicitly V2-centric

`crates/module-polymarket/src/eip712.rs:1–19` explicitly defines the file as Polymarket CLOB **V2** signing.

`eip712.rs:32–33` defines the V2 11-field order type.

`eip712.rs:135–168` defines `OrderV2`.

## 8.3 Current Polymarket protocol has moved forward

The official Polymarket Rust client currently supports V1, V2 and V3. Its current README states that `position_id` selects Exchange V3 automatically for position-backed orders.

The official changelog records:

- **0.8.0 — 2026-09-08:** V2 position IDs in Exchange V3 orders and market-data operations;
- **0.7.0 — 2026-07-17:** async execution pipeline handling `tradeIDs` and cases where transaction hashes are not immediately present.

## 8.4 Current project response gap

`crates/module-polymarket/src/clob.rs:158–184` defines `PostOrderResponse` with:

- order_id;
- success;
- error_msg;
- status;
- taking_amount;
- making_amount.

It does not expose a `trade_ids` field analogous to the current Polymarket async response schema.

The project may still reconcile later via existing local logic, but the response model is not current-protocol complete.

## 8.5 Strategy gap

`crates/module-polymarket/src/strategy.rs:150` explicitly states the configured strategy name is not implemented.

Current strategy evaluation is limited to existing supported rule forms rather than a broad strategy catalog.

## 8.6 Polymarket assessment

**Current implementation completeness: ~75–85%.**

Primary gap:

- Exchange V3 position-backed order path;
- current async `tradeIDs` response handling;
- broader strategy framework;
- current live/funded compatibility evidence.

---

# 9. MODULE 4 — STAKING SMART CONTRACT

## 9.1 Present

`programs/staking-suite/src/processor.rs` now contains meaningful account validation:

- signer requirements;
- owner checks;
- config address checks;
- vault/mint/treasury binding;
- staker token mint/owner checks;
- supply-headroom enforcement;
- genesis-mint restrictions.

This is materially different from the old audit that called the account-validation layer critical and missing.

## 9.2 Remaining deployment gap

`programs/staking-suite/src/lib.rs:36` still carries a placeholder program ID.

Therefore:

- code exists;
- test/build evidence may exist historically;
- production identity/deployment is NOT complete in this snapshot.

## 9.3 Audit gap

No external independent smart-contract security audit is included in the repository evidence.

For a buyer putting real value at risk, this remains a separate external verification requirement.

## 9.4 Staking assessment

**Code completeness: ~85–92%.**

**Commercial deployment/audit readiness gap: ~20–35%.**

---

# 10. MODULE 5 — TELEGRAM CONTROL BOT

## 10.1 Present

`crates/module-telegram/src/commands.rs` supports control and status functions including:

- `/on`
- `/off`
- `/kill`
- `/resume`
- `/status`
- `/positions`
- `/trades`
- `/pnl`
- `/balance`
- `/mode`
- `/config`

RBAC and deny-by-default controls exist.

## 10.2 Product gap

The Telegram module is primarily a CONTROL BOT.

Leading products increasingly use Telegram as a complete trading/discovery surface, not merely ON/OFF control.

Therefore missing customer-product layers include:

- tenant-aware Telegram identity binding;
- per-tenant portfolio views;
- richer trading actions;
- copy-task UX;
- token/contract analytics;
- smart-money alerts;
- mobile-first product workflows.

## 10.3 Assessment

**Core control implementation: ~90%.**

**SaaS/trading-terminal surface gap: ~30–50%.**

---

# 11. TENANT ARCHITECTURE — CRITICAL REAL GAP

## 11.1 Foundation PRESENT

STEP-3 infrastructure now includes:

- tenant identity;
- tenant runtime records;
- runtime generation/fencing;
- tenant config;
- tenant bindings;
- execution authority;
- tenant gateway;
- tenant background context;
- tenant stream envelope;
- tenant observability.

This is real architecture work.

## 11.2 Module boundary NOT fully wired

Static search across these current source trees found **zero** occurrences of:

- `OrganizationId`
- `TenantExecutionContext`
- `TenantExecutionGateway`
- `TenantContext`
- `organization_id`

inside the production source of:

- `crates/module-sniper/src`
- `crates/module-copy/src`
- `crates/module-polymarket/src`
- `crates/module-telegram/src`
- `crates/solana-kit/src`

Therefore the tenant security model exists in the server/control layer, but is not yet propagated into the core money-moving module crates.

## 11.3 Concrete startup evidence

`crates/server/src/main.rs:850–863` defines `spawn_modules()` around process-level:

- `Shared` state;
- RPC;
- one wallet;
- signer registry;
- global config.

`main.rs:866–883` constructs Sniper using the global state/wallet/signers.

`main.rs:889–912` constructs Copy using the same state/wallet/signers.

`main.rs:918–930` constructs Polymarket from shared state.

`main.rs:937–946` spawns Telegram from shared state.

That is not the same as:

`tenant → isolated runtime → isolated wallet/signer → module instance`.

## 11.4 Assessment

**True tenant-runtime isolation gap: ~70–80%.**

This remains a **P0** for a real SaaS sale.

---

# 12. DATABASE TENANT ISOLATION — CRITICAL REAL GAP

## 12.1 Tenant helper code exists

`crates/core/src/db/tenant_query.rs` contains tenant-predicate helpers and tests.

However, production repositories still contain global SQL patterns.

## 12.2 `crates/core/src/db/repo.rs` — order paths

Relevant current lines include:

- `79–84`: orders insert and `ON CONFLICT (idempotency_key)`;
- `111+`: orders upsert;
- `166+`: update by order ID;
- `223`: order lookup by ID;
- `236`: order lookup by idempotency key;
- `250`: lookup by signature;
- `266+`: incomplete/global list;
- `281`: global order listing.

The repository methods do not consistently require a tenant parameter and do not consistently apply `organization_id` in the SQL predicate.

## 12.3 Positions/trades

Current repository patterns include:

- `385`: position upsert by `id`;
- `440`: open position list without tenant parameter;
- `454`: position by ID;
- `467`: global position list;
- `497`: trade insert;
- `528`: global trade list;
- `543`: trades by position ID.

## 12.4 Transactions/intents

Current patterns include:

- `763`: transaction status by signature;
- `792`: transaction upsert with signature uniqueness;
- `822`: transaction lookup by signature;
- `885`: transaction list;
- `948`: execution intent insert with `ON CONFLICT(intent_id)`;
- `1003`: intent by ID;
- `1023`: orphan-intent recovery list.

These identifiers may be globally meaningful in some cases, but tenant-sensitive callers must still prove ownership before disclosure or mutation.

## 12.5 Copy repository

`crates/core/src/db/copy.rs` has current global patterns:

- `123`: `ON CONFLICT(address)` for copy leaders;
- `158`: leader by address;
- `172`: global leader list;
- `210`: leader events by address;
- `237`: copy event upsert by `event_id`;
- `276`: copy event lookup;
- `296/318`: global copy-event queries;
- `343`: copy links `ON CONFLICT(position_id)`;
- `380`: link by position ID;
- `395`: open links globally;
- `419`: update by position ID.

## 12.6 Polymarket repository

`crates/core/src/db/polymarket.rs` currently has:

- `140`: signal insert/upsert;
- `184`: signal lookup;
- `203`: signal list;
- `224`: order insert;
- `272`: order by venue order ID;
- `288`: global order list;
- `307`: fill insert;
- `337`: fills by venue order ID;
- `354`: reconciliation insert;
- `379`: global reconciliation list.

## 12.7 Execution/recovery repository

`crates/core/src/db/execution.rs` includes:

- `38`: lifecycle insert;
- `106`: lifecycle by intent ID;
- `120`: lifecycle by signature;
- `137`: open lifecycle list;
- `152`: recent lifecycle list;
- `204`: global settled-delete path.

`crates/core/src/db/claims.rs` contains execution-claim reads/writes by execution ID and an `ON CONFLICT(execution_id)` path.

Execution IDs and blockchain signatures may be globally unique by domain; this does NOT remove the requirement for tenant ownership checks when a customer request reaches these methods.

## 12.8 Assessment

**Tenant-scoped trading DB gap: ~75–90%.**

The SQL schema has tenant columns, but the **application repository layer is not yet fully enforcing them**.

This is arguably the biggest technical blocker to calling the product a true multi-tenant trading SaaS.

---

# 13. REMOTE CUSTODY — REAL MISSING IMPLEMENTATION

## 13.1 Vault

`crates/core/src/custody/provider.rs:162–185`

The Vault provider is a deliberate unsupported implementation and returns `UnsupportedProvider`.

## 13.2 KMS

`provider.rs:187–208`

KMS is also unsupported.

## 13.3 HSM

`provider.rs:210–231`

HSM is also unsupported.

## 13.4 Config confirms this

`crates/core/src/config.rs:1507–1524` explicitly states only local signing is implemented; Vault/KMS/HSM are not implemented in this build.

## 13.5 Server custody path confirms fail-closed unsupported-provider behavior

`crates/server/src/saas/custody.rs:779–784` returns `NOT_IMPLEMENTED` for non-local provider types rather than silently falling back.

This is security-safe behavior, but it is still a **missing commercial capability**.

## 13.6 Rotation gap

`crates/server/src/saas/custody_rotation.rs:84–87` uses a placeholder/synthetic custody profile resolution.

## 13.7 Assessment

**Remote custody implementation gap: ~60–70%.**

Local signer = PRESENT.  
Remote Vault/KMS/HSM = NOT PRESENT.

## 13.8 Post-audit remediation (2026-09-30)

The findings above are preserved as written for audit integrity (audit
date 2026-09-29). The following were remediated in this tree AFTER that
date, by the PROMPT 5 custody work:

* **§13.1 Vault:** a real transit-engine adapter now exists at
  `crates/server/src/custody/vault/` (REST client `sys/health`,
  `token/lookup-self`, `transit/keys`, `transit/sign`; signer, health,
  config; token in a redacted wrapper). Unit-tested; no live round-trip.
* **§13.2 KMS:** a real SigV4-signed AWS KMS adapter now exists at
  `crates/server/src/custody/kms/` (`TrentService.GetPublicKey` +
  `TrentService.Sign`, Ed25519 `EDDSA_SHA_512`; SigV4 verified against the
  AWS-documented test vector). Unit-tested; no live round-trip.
* **§13.5 server custody path:** remote resolution/signing now flows
  through the custody sign boundary (`sign_boundary.rs`) with the
  deployment provider registry; the legacy control-plane resolve endpoint
  still refuses remote providers fail-closed (no local fallback).
* **§13.6 rotation gap:** `POST /api/saas/custody/rotations` now resolves
  the REAL custody profile from the authenticated organization; synthetic
  profile ids are impossible.
* **§13.3 HSM / §13.4 config:** unchanged — HSM remains an explicitly
  fail-closed unimplemented provider; `[signing] provider = vault/kms/hsm`
  (the solana-kit transaction-signer layer) still fails startup with
  `SignerError::UnsupportedBackend`.

§13.7 superseded: remote Vault/KMS = PRESENT (implemented, unit-tested,
NOT live-proven); HSM = NOT PRESENT. Current state:
`docs/CUSTODY-STATUS-2026.md`.

---

# 14. BILLING / COMMERCIAL STATE — PARTIAL, NOT FINISHED

## 14.1 Billing status endpoint

`crates/server/src/saas/billing_status.rs:97–124` currently synthesizes state.

Notable current lines:

- `107`: placeholder plan lookup;
- `108`: fallback to `starter`;
- `109`: dunning placeholder;
- `114`: `subscription_status = active`;
- `115`: billing provider = `manual`;
- `116–117`: payment/invoice state = null;
- `119`: usage totals = zero;
- `120`: dunning state = Current.

## 14.2 Usage limits

`crates/server/src/saas/usage_limits.rs:77–115` explicitly documents production DB lookup as future behavior and uses deterministic synthetic values for the handler path.

Current source fixes the plan to `pro` and uses sample usage values such as 42 monthly orders and 2 members.

## 14.3 Payment event application

`crates/server/src/saas/payment_webhooks.rs` has a durable/event-oriented structure, but the current code still contains an audit-only branch for payment-state application and a custom deterministic hashing helper.

`payment_webhooks.rs:388` literally describes the current helper as a placeholder based on truncated SHA-256.

## 14.4 Product documentation says the same thing

`docs/SAAS-PRODUCT.md:38–48` explicitly states:

- only manual billing provider is implemented;
- Stripe/Paddle contract exists but returns `501` until adapter/secret are configured;
- no self-service checkout;
- no invoicing;
- no tax handling;
- no card data.

This document is honest; the problem is that some sales material could be interpreted as if SaaS billing were complete.

## 14.5 Assessment

**Commercial billing gap: ~40–60%.**

## 14.6 Post-audit remediation (2026-09-30)

The findings above are preserved as written (audit date 2026-09-29). Since
then, PROMPT 5 remediated them: `billing_status.rs` renders through the
authoritative `BillingView` (real subscription/plan/usage — nothing
synthesized); `usage_limits.rs` was fully rewritten after the final
acceptance audit found its synthetic `42.0`/`2.0` usage + hardcoded `pro`
plan (criterion I) — every number is now store-derived and a
no-subscription tenant sees `plan_code:"none"`; REAL Stripe/Paddle
adapters exist (`billing/{stripe,paddle}_adapter.rs`: signature-verified,
idempotent webhooks) plus `POST /api/saas/checkout` (idempotent) and
`GET /api/saas/invoices`. No live provider round-trip is claimed
(GAP-001: `LIVE_BILLING=1` + real credentials, buyer-side).

---

# 15. CUSTOMER CONTROL-PLANE / UI GAP

## 15.1 Actual page tree

Current `apps/control-plane/src/app/` contains only:

- `page.tsx`
- `billing/page.tsx`
- `custody/page.tsx`
- `settings/data-lifecycle/page.tsx`
- `layout.tsx`

There is **no current customer trading page** in the app tree.

## 15.2 API exposure

`apps/control-plane/src/lib/api.ts` exposes tenant APIs for:

- user/session;
- organizations;
- API keys;
- wallet access;
- exports;
- and operator APIs for status/orders/positions/risk/portfolio/reconciliation/HA.

The operator APIs include:

- `/api/orders`
- `/api/positions`
- `/api/risk/global`
- `/api/accounting/portfolio`
- `/api/reconciliation/findings`
- `/api/ha`

But there is no equivalent rich customer trading dashboard consuming tenant-scoped trading truth.

## 15.3 Product documentation confirms limitation

`docs/SAAS-PRODUCT.md:50–83` describes the tenant console as a multi-section control plane but explicitly states that the seven trading sections show an explanatory notice and that trading truth is not re-served per tenant by the SaaS layer.

## 15.4 Assessment

**Customer trading UX gap: ~60–75%.**

## 15.5 Post-audit remediation (2026-09-30)

The findings above are preserved as written (audit date 2026-09-29).
PROMPT 5 remediated them: eight customer trading pages now exist under
`apps/control-plane/src/app/trading/*` (overview, orders, positions,
executions, sniper, copy, polymarket, telegram) served by the tenant
data plane (`/api/tenant/*`, 21 authenticated routes) through
`lib/customer-trading-api.ts`, which refuses any path outside
`/api/tenant/*`. Trading truth IS now re-served per tenant through the
tenant-scoped trading repositories (every SQL predicate carries
`organization_id`; cross-tenant = not-found). The operator console keeps
its own global view.

---

# 16. POLYMARKET CURRENT-PROTOCOL GAP — EXTERNAL VERIFICATION

Official Polymarket sources current around this audit date show:

- V1/V2/V3 protocol support in the maintained Rust client;
- position-backed V2 orders use Exchange V3 signing automatically;
- 2026-09-08 release adds V3 position IDs;
- 2026-07-17 release addresses async execution and `tradeIDs` response behavior.

The project is still explicitly centered on `OrderV2` and a response model without `trade_ids`.

### Required next implementation

Add:

1. explicit V3 position-backed order builder/signing;
2. current protocol selection logic;
3. `tradeIDs` wire parsing;
4. async trade lookup/backfill;
5. tests against current wire examples;
6. regression coverage for existing V2 behavior.

### Post-audit remediation (2026-09-30)

All six items above were delivered by PROMPTs 4–5: `exchange_v3.rs`
(domain `v"3"` `0xe33337…`, 11-field struct, `positionID` XOR-enforced
with `tokenID`), version selection in `orders.rs`/`eip712.rs`,
`tradeIDs` alias-parsing, `async_commit.rs` + `trade_resolution.rs` +
`backfill.rs` + `reconcile_async.rs`, wire-example signing tests, and
the V2 regression suite (all green in the 3312-test workspace run;
`PROMPT-5-RESULT.md` §2).

---

# 17. RAYDIUM / SOLANA LOW-LATENCY GAP — EXTERNAL VERIFICATION

Current Solana production documentation says:

- priority fees increase scheduling priority;
- public RPC is not suitable as a production SLA path;
- fresh blockhash and expiration tracking matter;
- `skipPreflight=true` is recommended after prior validation when optimizing for lowest latency;
- compute-budget configuration matters.

Jito documentation states:

- 50ms parallel auctions;
- low-latency transaction forwarding;
- bundles up to five transactions;
- `sendTransaction` uses `skip_preflight=true`;
- successful submission does NOT by itself guarantee on-chain landing;
- bundle/transaction status must be tracked.

Your project already has priority-fee/Jito support, so those are **not missing features**.

The real remaining gap is **proof + adaptive production optimization**, not mere presence of a Jito flag.

Required evidence for a serious sales claim:

- p50/p95 detection latency;
- p50/p95 build/sign latency;
- p50/p95 submit latency;
- p50/p95 landing latency;
- landed vs expired vs dropped ratio;
- results by RPC region;
- results by priority fee/Jito tip band;
- results under congestion.

---

# 18. COMPETITOR / OTHER SOFTWARE BUSINESS GAP

## 18.1 Trojan

Current Trojan public materials include:

- Copy Trading;
- Wallet Analyzer;
- Token Audits;
- Real-Time Alerts;
- historical on-chain data;
- Watchlists;
- mobile trading/portfolio workflows.

Your project has a deeper custom Rust execution/infrastructure angle but does not currently match that customer analytics/discovery surface.

### Gap

- wallet analytics;
- token audit/reputation layer;
- market discovery UI;
- rich alerts;
- customer mobile terminal.

## 18.2 GMGN

Current GMGN documentation includes:

- wallet tracking;
- real-time smart-money activity;
- Copy Trade;
- automatic buy/sell / TP/SL behavior;
- Wallet Radar;
- Telegram alert tooling;
- wallet performance/discovery workflows.

### Gap

Your Copy engine is substantial, but your customer-facing discovery and analytics layer is significantly thinner.

## 18.3 3Commas

Current 3Commas documentation allows up to:

- 50 API accounts;
- 10 exchanges;
- 10 trading pairs

within its documented Signal Bot / Terminal limits.

Your project is specialized around Solana/Polymarket rather than general multi-exchange trading.

### Gap

This is mostly a **scope/breadth difference**, not automatically a defect.

Do not market the product as a general multi-exchange terminal unless that capability is actually added.

## 18.4 Hummingbot

Current Hummingbot Dashboard documentation describes:

- multiple exchanges;
- portfolio management;
- strategy configuration;
- backtesting;
- deployment/management of multiple bot instances;
- API-driven operation.

### Gap

Your system lacks comparable customer-facing:

- backtesting workflow;
- strategy catalog;
- bot deployment UX;
- broad exchange connectors;
- portfolio analytics surface.

Again, this is scope breadth rather than a defect if your product is intentionally Solana + Polymarket specialized.

---

# 19. POLYMARKET / STRATEGY BUSINESS GAP

`module-polymarket/src/strategy.rs:150` explicitly says the configured strategy name is not implemented.

Current supported strategy behavior is narrow relative to a general trading platform.

Recommended productization later:

- strategy registry;
- versioned strategies;
- parameter schema;
- dry-run evaluation;
- backtest;
- replay;
- deploy/stop;
- per-tenant strategy ownership;
- strategy performance analytics.

Priority: **P1/P2**, after tenant data-plane and custody.

---

# 20. SECURITY / EVIDENCE GAPS

## 20.1 Audit attestation

`crates/server/src/ops/audit_attestation.rs:19` still describes its signature field as allowing a deterministic placeholder/detached-signature concept.

The source later contains HMAC code, so this is partially implemented cryptographically but the public data model/documentation is not cleanly separated between:

- actual HMAC verification;
- future asymmetric/detached signature support.

Buyer evidence should use unambiguous terminology.

## 20.2 Zeroize

Current `AUDIT.md` remediation history records `zeroize` as not done for config-held String secrets.

This is not equivalent to “plaintext keys are exposed”; the existing Solana `Keypair` path already has zeroization behavior. But a buyer-facing hardened build may still want secret-memory lifecycle reviewed.

## 20.3 External audit

No independent application/contract security audit is included.

This must not be marketed as:

- fully audited;
- security certified;
- institutional production verified.

---

# 21. LEGAL / IP / COMMERCIAL DUE-DILIGENCE GAPS

## 21.1 LICENSE holder

`LICENSE:3` still says:

`Copyright (c) 2026 sniper-suite authors`

`LICENSE:25–27` explicitly tells the transferring party to replace the generic holder line.

That means legal ownership is not fully finalized in the repository itself.

## 21.2 Repository identity

The IP register records the lack of a finalized repository URL.

## 21.3 Third-party license uncertainty

`docs/IP-OWNERSHIP-REGISTER.md` records `UNKNOWN` third-party licenses where dependency metadata is incomplete.

These are legal-review items, not proof of infringement.

## 21.4 Staking identity

Program ID remains placeholder and deploy authority/keypair is intentionally not committed.

## 21.5 Branding

Trademark/domain ownership is explicitly not included.

### Priority

P1 legal closeout before a signed commercial IP-transfer deal.

---

# 22. BUSINESS / PRODUCT GAPS BEYOND CODE

These are important for a real $20k–$60k sale because buyers often evaluate operating burden, not LOC.

## P0

1. Current buyer package must exactly match canonical source.
2. True tenant-to-module runtime wiring.
3. Tenant-aware production DB repositories.
4. Real remote custody where promised.
5. Current Polymarket protocol compatibility.
6. Current buyer-facing evidence and manifests.

## P1

7. Customer trading dashboard.
8. Real billing state and entitlement enforcement.
9. Stripe/Paddle real adapter flows if self-service SaaS is sold.
10. Smart-contract external audit.
11. Staking deployment identity.
12. Live/testnet funded verification.
13. latency/landing-rate dashboard.
14. customer alerts and operational SLOs.
15. mobile/PWA customer surface.

## P2

16. Wallet Analyzer.
17. Token audit/risk discovery UI.
18. smart-money discovery.
19. backtesting.
20. strategy marketplace/catalog.
21. multi-exchange expansion.
22. white-label/reseller mode.
23. affiliate/referral infrastructure.
24. support ticket / customer success tooling.

---

# 23. WHAT IS ALREADY VALUABLE — DO NOT REWRITE

The following components represent substantial implementation value:

1. `crates/solana-kit/`
   - RPC/WS infrastructure;
   - transaction construction/signing;
   - Pump.fun/PumpSwap/Raydium/Jupiter integration;
   - Jito path;
   - caching/retry/failover.

2. `crates/module-sniper/`
   - launch detection;
   - risk pipeline;
   - route selection;
   - ownership claims;
   - execution integration.

3. `crates/module-copy/`
   - wallet feed handling;
   - copy sizing;
   - staleness;
   - execution/recovery infrastructure.

4. `crates/module-polymarket/`
   - Gamma/CLOB/WS;
   - L1/L2 auth;
   - vector-style EIP-712 V2 implementation;
   - lifecycle/reconciliation structures.

5. `bot-core`
   - risk engine;
   - OMS;
   - idempotency;
   - intent journal;
   - accounting;
   - HA/distributed ownership.

6. `crates/server/src/tenant/`
   - tenant execution foundation;
   - runtime registry;
   - fencing;
   - authorization gateway;
   - tenant config.

7. `programs/staking-suite/`
   - meaningful native Solana program;
   - staking/reward/fee/admin flows;
   - current account validation.

These are **hard-won implementation surfaces** and should be extended, not replaced without evidence.

---

# 24. SAFE VS UNSAFE MARKETING

## 24.1 SAFE claims supported by the current code

- Rust-based modular crypto trading infrastructure.
- Axum control plane.
- Solana SDK-based execution stack.
- Pump.fun launch detection.
- PumpPortal + Geyser + Solana log-based feeds.
- PumpSwap/Raydium AMM v4/Jupiter execution routes.
- Priority-fee/Jito submission support.
- Copy trading for tracked Solana wallets.
- Polymarket Gamma/CLOB REST/WS with EIP-712 V2 signing.
- Native Solana staking program with staking/reward/fee/admin controls.
- Telegram bot for module control and monitoring.
- PostgreSQL/Redis/HA/recovery/audit infrastructure.
- Tenant identity/runtime/authorization foundation.

## 24.2 Claims that should NOT be made today

Do not say:

- “Guaranteed buy in under 1 second.”
- “Guaranteed transaction landing.”
- “Profitable bot.”
- “AI predicts winning coins.”
- “Fully production-ready SaaS.”
- “True fully isolated multi-tenant trading platform.”
- “Vault/KMS/HSM custody included.”
- “Self-service Stripe/Paddle billing included.”
- “All Raydium protocols supported.”
- “Latest Polymarket V3 fully supported.”
- “Fully audited.”
- “Mainnet proven.”
- “Institutional-grade SLA.”
- “Zero-risk / risk-free.”

Those claims exceed the current source/evidence.

---

# 25. RECOMMENDED CURRENT SALES POSITIONING

## 25.1 $20k class positioning

Position as:

> Advanced Rust crypto-trading infrastructure/IP package covering Solana launch sniping, wallet copy trading, Polymarket automation, native staking and Telegram control, with risk, persistence, recovery and HA foundations.

Required buyer disclosure:

- single-operator or partially productized SaaS;
- external audit not included;
- live funded trading evidence not established in the current audit environment;
- remote custody backends not implemented;
- customer trading SaaS layer incomplete;
- buyer-release package must be regenerated before handover.

## 25.2 $40k class positioning

A $40k sales narrative becomes stronger after:

- tenant data plane wired;
- live tenant isolation tests against PostgreSQL;
- current Polymarket compatibility;
- current release package parity;
- customer trading dashboard;
- real billing/entitlement state;
- testnet/devnet funded proof where available;
- stronger operational evidence;
- contract external audit or third-party review.

## 25.3 $60k class positioning

A credible `$60k enterprise` narrative should require at minimum:

- true tenant-isolated runtime;
- tenant-aware money-moving persistence;
- production remote custody backend(s);
- authoritative billing and entitlements;
- customer-facing trading console;
- current Polymarket V3/async support;
- independent application/contract security review;
- fresh acceptance evidence;
- landing-rate evidence;
- hardened deployment/IaC/DR/SLOs;
- finalized IP/license paperwork.

This is a sales-readiness roadmap, not an independent appraisal of market value.

---

# 26. FINAL GAP SCORECARD

| Area | Current state | Gap |
|---|---|---:|
| Sniper core | STRONG | 5–10% |
| Copy core | STRONG | 5–10% |
| Polymarket current compatibility | PARTIAL | 15–25% |
| Staking code | STRONG/PARTIAL deployment | 8–15% code, 20–35% commercial |
| Telegram control | STRONG | 5–10% |
| Risk/OMS/recovery | STRONG | 5–15% |
| HA/distributed | STRONG foundation | 10–20% |
| Tenant domain/foundation | PRESENT | 10–20% foundation gap |
| Tenant runtime wiring | PARTIAL | 70–80% |
| Tenant trading DB enforcement | PARTIAL | 75–90% |
| Remote custody | MISSING except local | 60–70% |
| Billing authoritative state | PARTIAL | 40–60% |
| Customer trading UI | MISSING/limited | 60–75% |
| Competitor analytics/discovery | LIMITED | 50–80% |
| Release package parity | FAILING CURRENTLY | P0 |
| Documentation freshness | INCONSISTENT | P0 |
| External audit | MISSING | 100% |
| Funded live proof | MISSING/UNVERIFIED | 100% evidence gap |
| Legal/IP finalization | PARTIAL | 20–40% |

---

# 27. PRIORITY ORDER TO REDUCE THE REAL $20K–$60K GAP

## P0-1
**Regenerate and verify the buyer package.**

Canonical tree → buyer-release/source → release-manifest → checksums → verification.

No commercial handover before `verify-buyer-package.sh` passes against the exact bundle.

## P0-2
**Wire tenant identity into actual trading modules.**

TenantExecutionContext must reach:

`Sniper / Copy / Polymarket → order → risk → intent → sign → broadcast → persistence → reconciliation`.

## P0-3
**Replace tenant-blind production repository access.**

All tenant-sensitive SELECT/UPDATE/DELETE/UPSERT/recovery/reporting operations must use explicit tenant scope.

## P0-4
**Complete atomic tenant-local uniqueness transitions.**

Schema and writer `ON CONFLICT` arbiters must be changed together.

## P0-5
**Current Polymarket compatibility.**

V3 position-backed orders + `tradeIDs` async response handling.

## P1-1
**Real remote custody.**

At least one production remote custody provider can be implemented cleanly without compromising the provider-neutral boundary.

## P1-2
**Authoritative billing.**

No synthetic plan/usage/payment status in production path.

## P1-3
**Customer trading dashboard.**

Tenant-specific orders, positions, PnL, bots, copy and Polymarket views.

## P1-4
**Security review + contract audit.**

Independent review and published remediation evidence.

## P1-5
**Live/testnet and latency evidence.**

Measure, do not promise.

---

# 28. WHAT THE CURRENT PROJECT CAN HONESTLY SAY

### Current truthful one-line description

> `sniper-suite` is a Rust-based modular crypto-trading infrastructure stack combining Solana launch sniping, copy trading, Polymarket automation, a native staking program and Telegram control, backed by risk, persistence, recovery, HA and a growing tenant/SaaS control plane.

### Current truthful buyer description

> The asset is best understood as a substantial specialized trading-engine/IP acquisition with a partially implemented SaaS control plane, rather than a finished general-purpose trading SaaS.

### Current truthful $20k–$60k statement

> The codebase has enough specialized engineering depth to support a serious commercial transaction, but the upper end of the requested range depends on closing the tenant data-plane, runtime isolation, remote custody, billing, customer UX, current protocol compatibility, independent security review and release-package integrity gaps documented here.

---

# 29. EXTERNAL SOURCES USED FOR THE CURRENT BENCHMARK

Current public sources consulted around 2026-09-29:

1. **Solana Documentation — Fees**
   - priority fee increases scheduling priority; no guarantee of landing.

2. **Solana Documentation — Production Readiness**
   - production RPC requirements, priority fees, fresh blockhashes, expiration tracking, transaction send configuration, `skipPreflight` guidance.

3. **Jito Labs Documentation — Low Latency Transaction Send**
   - 50ms auctions, direct validator forwarding, bundles, tip/priority considerations, status tracking.

4. **Polymarket / rs-clob-client-v2 README**
   - V1/V2/V3 protocol support; `position_id` selects V3 for position-backed orders.

5. **Polymarket / rs-clob-client-v2 CHANGELOG**
   - 0.8.0 (2026-09-08) V3 position IDs; 0.7.0 (2026-07-17) async `tradeIDs` behavior.

6. **Raydium Documentation**
   - current coverage includes AMM v4, CPMM, CLMM, Farm/Staking, LaunchLab and Perps.

7. **Trojan public product/blog material**
   - copy trading, Wallet Analyzer, Token Audits, alerts, historic data, watchlists, mobile workflows.

8. **GMGN public documentation**
   - wallet tracking, copy trading, smart-money tracking, Wallet Radar, Telegram alerts.

9. **3Commas Help Center**
   - documented multi-exchange/API/pair scale limits.

10. **Hummingbot Dashboard documentation**
    - strategy configuration/backtesting and multiple bot deployment/management.

---

# 30. FINAL AUDIT CONCLUSION

## Technical

The core trading infrastructure is materially more complete than the old root audit suggests.

## SaaS

The tenant control-plane foundation is real, but the **actual trading data-plane is not yet fully tenant-isolated**.

## Security

Major older defects such as staking account-validation and dashboard/API security hardening have been remediated according to later repository history, but remote custody and independent security review remain incomplete.

## Protocol freshness

Polymarket is behind the current official client protocol surface because the project is centered on V2 order signing and does not yet expose the complete current V3/async response model.

## Product/business

The project lacks the customer analytics/discovery/UI breadth of leading Solana terminals and broad trading platforms.

## Packaging

The current buyer-release mirror is **not cleanly synchronized** with the canonical source tree. This must be fixed before any buyer receives the package.

## Marketing

Market the asset as specialized, fast, modular trading infrastructure with documented limits. Do not market guarantees, profitability, complete SaaS isolation, current V3 compatibility, remote custody, or full audit status unless those are actually implemented and evidenced.

## Current headline number

**For the exact five-module engineering scope: ~8–12% gap.**

**For a true enterprise multi-tenant `$20k–$60k SaaS`: ~40–48% gap.**

**For broad competitor/product parity: ~50–65% gap.**

These numbers should be updated after the P0 tenant data-plane and package-parity work is actually completed and independently validated.

---

# APPENDIX A — HIGH-VALUE CURRENT FILE/LINE FINDINGS

1. `crates/server/src/main.rs:850–863` — process-level `spawn_modules` inputs.
2. `crates/server/src/main.rs:866–883` — Sniper gets process/global wallet/signers.
3. `crates/server/src/main.rs:889–912` — Copy gets process/global wallet/signers.
4. `crates/server/src/main.rs:918–930` — Polymarket shared process state.
5. `crates/server/src/main.rs:937–946` — Telegram shared process state.
6. `crates/core/src/db/repo.rs:79–84` — order insert and global idempotency conflict.
7. `crates/core/src/db/repo.rs:223` — order lookup by ID.
8. `crates/core/src/db/repo.rs:236` — order lookup by idempotency key.
9. `crates/core/src/db/repo.rs:250` — order lookup by signature.
10. `crates/core/src/db/repo.rs:266–281` — global order lists.
11. `crates/core/src/db/repo.rs:385` — position upsert.
12. `crates/core/src/db/repo.rs:440–467` — global position reads.
13. `crates/core/src/db/repo.rs:497–543` — global trade reads.
14. `crates/core/src/db/repo.rs:763–885` — transaction reads/writes.
15. `crates/core/src/db/repo.rs:948–1023` — execution intent writes/recovery.
16. `crates/core/src/db/copy.rs:123–172` — global leader semantics.
17. `crates/core/src/db/copy.rs:230–318` — global copy-event semantics.
18. `crates/core/src/db/copy.rs:343–419` — copy-link global access.
19. `crates/core/src/db/polymarket.rs:224–379` — global Polymarket order/fill/reconciliation access.
20. `crates/core/src/db/execution.rs:106–204` — lifecycle recovery/global lists.
21. `crates/core/src/custody/provider.rs:162–185` — Vault unsupported.
22. `crates/core/src/custody/provider.rs:187–208` — KMS unsupported.
23. `crates/core/src/custody/provider.rs:210–231` — HSM unsupported.
24. `crates/core/src/config.rs:1507–1524` — local-only signing implementation statement.
25. `crates/server/src/saas/custody.rs:779–784` — remote provider returns `NOT_IMPLEMENTED`.
26. `crates/server/src/saas/custody_rotation.rs:84–87` — synthetic custody-profile resolution.
27. `crates/server/src/saas/billing_status.rs:97–124` — synthetic billing rendering.
28. `crates/server/src/saas/usage_limits.rs:77–115` — synthetic plan/usage values.
29. `crates/server/src/saas/payment_webhooks.rs:388` — placeholder hashing helper.
30. `crates/server/src/ops/audit_attestation.rs:19` — placeholder/detached-signature wording.
31. `crates/module-polymarket/src/eip712.rs:1–19` — V2-only description.
32. `crates/module-polymarket/src/eip712.rs:32–33` — V2 typehash.
33. `crates/module-polymarket/src/eip712.rs:135–168` — `OrderV2`.
34. `crates/module-polymarket/src/clob.rs:158–184` — current order response fields, no `trade_ids` field.
35. `crates/module-polymarket/src/strategy.rs:150` — configured strategy name not implemented.
36. `crates/module-sniper/src/pipeline.rs:452–464` — direct entry-route set.
37. `crates/module-sniper/src/entry.rs:243–340` — precheck/risk/ownership pipeline.
38. `programs/staking-suite/src/lib.rs:36` — placeholder program ID.
39. `docs/SAAS-PRODUCT.md:38–48` — billing reality.
40. `docs/SAAS-PRODUCT.md:50–83` — tenant console/trading truth limitation.
41. `apps/control-plane/src/lib/api.ts:248–255` — operator trading API endpoints.
42. `LICENSE:25–27` — legal holder handover requirement.
43. `docs/IP-OWNERSHIP-REGISTER.md:26–36` — unresolved legal/IP review items.
44. `README.md:9–13` — current 5-module marketing claims that need careful wording.
45. `scripts/verify-buyer-package.sh` — current buyer-release parity failures.

---

# APPENDIX B — FINAL NO-SKIP AUDIT NOTE

This report is a comprehensive re-audit of the uploaded workspace using complete-tree inventory, line-numbered source inspection, targeted semantic review of all money-moving/security/commercial paths, SQL pattern scanning, release-package verification, and current public-market/protocol comparison.

It does NOT claim that every one of the 183,640 Rust lines was manually read character-by-character by a human. No such claim would be technically honest.

The exact current build/test status must be verified from the seller's pinned Rust toolchain environment before final buyer handover because `cargo` was unavailable in this audit environment.

END OF AUDIT.
