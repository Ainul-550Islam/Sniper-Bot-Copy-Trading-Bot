# PROMPT 5/10
# NEXT MAJOR BUYER-GAP CLOSURE
# POLYMARKET V3 + ASYNC
# CUSTODY
# BILLING / ENTITLEMENT
# CUSTOMER TRADING API/UI
# BUYER PACKAGE PARITY
# 60+ FILE TARGET
# DO NOT SKIP ANY CODE

========================================================
PROJECT
========================================================

Existing:

sniper-suite

Tech:
- Rust
- Axum
- Solana SDK
- Telegram API
- PostgreSQL
- Redis
- Next.js control plane

Modules:
1. Sniper
2. Copy Trading
3. Polymarket
4. Staking/Token/Fee
5. Telegram

========================================================
CURRENT VERIFIED BASELINE
========================================================

PROMPT 2/3 already built tenant foundation:
- TenantExecutionContext
- tenant authorization gateway
- tenant runtime
- fencing
- tenant configuration
- tenant bindings
- background/stream isolation
- tenant-aware observability
- tenant-scoped data-plane foundations

PROMPT 4 has now completed and verified:
- `TenantSigningContext`
- `TenantTransactionMeta`
- `TenantBroadcastGuard`
- Sniper tenant execution
- Copy tenant execution
- exit/sweeper guards
- tenant-local dedup
- repository-backed execution/copy sinks
- exact submitted vs confirmed persistence semantics
- live PostgreSQL factory verification
- operator mode preservation

Latest reported PROMPT 4 validation:
- solana-kit: 294 passed
- module-sniper: 144 passed
- module-copy: 110 passed
- sniper-suite: 1288 passed
- bot-core: 727 passed
- module-polymarket: 147 passed
- module-telegram: 21 passed
- saas-sdk: 32 passed
- workspace-wide reported: 2763 passed, 0 failed
- workspace check clean
- fmt clean
- clippy clean

PROMPT 4 explicitly left:
- Polymarket V3/async implementation for the next prompt
- real Vault/KMS/HSM implementation still absent
- billing/customer SaaS productization still incomplete

The latest result also explicitly says:
- local signer remains the only implemented custody backend;
- Polymarket current protocol work remains queued;
- no fake remote custody was added.

========================================================
PRIMARY OBJECTIVE
========================================================

This prompt MUST close the next large commercial gap:

A. Current Polymarket protocol compatibility
B. Real asynchronous order lifecycle handling
C. Tenant-safe Polymarket execution path
D. Production custody provider architecture
E. Authoritative billing/usage/entitlement state
F. Customer trading API
G. Customer trading dashboard
H. Buyer-release source parity
I. Current buyer-facing evidence
J. Marketing/documentation correctness

Do NOT rewrite Sniper or Copy.

========================================================
NON-NEGOTIABLE RULES
========================================================

1. Do not delete working code.

2. Do not rewrite the existing Sniper engine.

3. Do not rewrite the existing Copy engine.

4. Do not rewrite the tenant execution boundary.

5. Reuse:
- TenantExecutionContext
- TenantSigningContext
- TenantBroadcastGuard
- TenantRuntime
- TenantModuleFactory
- existing repository abstractions
- existing billing abstractions
- existing custody provider model
- existing control-plane auth

6. No placeholder implementation.

7. No fake provider success.

8. No fake billing state.

9. No synthetic usage.

10. No hardcoded customer plan.

11. No hardcoded customer organization.

12. No fake V3.

13. No fake trade IDs.

14. No fake custody signatures.

15. No fake customer metrics.

16. No:
`TODO`

17. No:
`FIXME`

18. No:
`stub`

19. No:
`// existing code`

20. No:
`// ...`

21. No:
`/* omitted */`

22. No:
`for brevity`

23. No:
`rest unchanged`

24. Every NEW file must contain complete code.

25. Every MODIFIED existing file must be returned with COMPLETE file content.

26. Never provide diff-only output.

27. Never skip imports.

28. Never skip test code.

29. Never skip SQL.

30. Preserve all existing tests and behavior.

31. Every new security-sensitive failure must fail closed.

32. Never leak private keys.

33. Never store raw custody credentials in tenant config.

34. Never let customer API choose arbitrary `organization_id`.

35. Tenant must come from authenticated principal/session/API-key binding.

36. Do not silently downgrade V3 to V2.

37. If a provider capability is unavailable, return an explicit machine-readable unavailable state.

38. Do not claim live provider integration unless it is really implemented and tested.

39. Do not claim independent security audit.

40. Do not claim production funded trading unless evidence exists.

========================================================
PHASE 0 — FORENSIC AUDIT BEFORE WRITING
========================================================

Search the current repository for:

Polymarket:
`OrderV2`
`OrderV3`
`position_id`
`positionId`
`tradeIDs`
`trade_ids`
`transactionsHashes`
`transactions_hashes`
`createOrder`
`postOrder`
`post_orders`
`FAK`
`FOK`
`GTC`
`GTD`
`async`
`reconcile`
`backfill`

Custody:
`SignerProvider`
`UnsupportedProvider`
`Vault`
`KMS`
`HSM`
`RemoteSigner`
`sign_message`
`sign_transaction`

Billing:
`billing_status`
`usage_limits`
`payment_webhooks`
`subscription`
`invoice`
`payment_transaction`
`entitlement`
`usage`

Customer API:
`/api/orders`
`/api/positions`
`/api/risk`
`/api/reconciliation`
`organizations`
`session`
`api_keys`

UI:
`apps/control-plane/src/app`
`operator`
`tenant`
`trading`

Buyer package:
`buyer-release`
`release-manifest`
`verify-buyer-package`
`verify-delivery`

Docs:
`SELLER-FACT-SHEET`
`FINAL-BUYER-STATUS`
`AUDIT.md`
`SAAS-PRODUCT.md`

For every finding classify:
- current
- partial
- stale
- duplicate
- production path
- operator path
- customer path
- missing
- external provider dependency

========================================================
TARGET
========================================================

TARGET = 68 NEW FILES.

If an exact file already exists:
DO NOT DUPLICATE IT.

Instead:
- extend the existing file;
- return full modified content;
- explain why it replaces the planned new file.

========================================================
A. POLYMARKET ORDER MODEL
========================================================

1.
`crates/module-polymarket/src/protocol/model.rs`
# Canonical protocol-level order model supporting current V2 and V3 semantics without duplicating existing order/domain types.

2.
`crates/module-polymarket/src/protocol/version.rs`
# Explicit protocol-version enum and selection rules for V1/V2/V3 behavior.

3.
`crates/module-polymarket/src/protocol/position.rs`
# Position-backed order model using current position_id semantics.

4.
`crates/module-polymarket/src/protocol/amounts.rs`
# Exact amount/decimal conversion helpers for current CLOB wire semantics.

5.
`crates/module-polymarket/src/protocol/timestamps.rs`
# Timestamp/expiration/nonce normalization for current order versions.

6.
`crates/module-polymarket/src/protocol/mod.rs`
# Protocol export surface.

========================================================
B. V3 SIGNING
========================================================

7.
`crates/module-polymarket/src/v3/model.rs`
# Exact V3-specific signing/order data structures.

8.
`crates/module-polymarket/src/v3/type_hash.rs`
# V3 EIP-712 type-hash/constants derived from the actual selected protocol implementation.

9.
`crates/module-polymarket/src/v3/domain.rs`
# V3 domain separator construction.

10.
`crates/module-polymarket/src/v3/sign.rs`
# Complete V3 signing path using existing cryptographic primitives.

11.
`crates/module-polymarket/src/v3/verify.rs`
# Local verification of generated signatures and typed-data inputs.

12.
`crates/module-polymarket/src/v3/order_builder.rs`
# V3 position-backed order builder.

13.
`crates/module-polymarket/src/v3/mod.rs`
# V3 exports.

14.
`crates/module-polymarket/tests/v3_signing.rs`
# V3 signing vectors/regression tests.

15.
`crates/module-polymarket/tests/v3_position_orders.rs`
# Position-backed order construction and serialization tests.

========================================================
C. POLYMARKET ASYNC EXECUTION
========================================================

16.
`crates/module-polymarket/src/async_exec/model.rs`
# Async order lifecycle states and wire-response model.

17.
`crates/module-polymarket/src/async_exec/response.rs`
# Parse current response fields including optional transaction hashes and trade IDs.

18.
`crates/module-polymarket/src/async_exec/trade_ids.rs`
# Exact trade-ID collection/normalization.

19.
`crates/module-polymarket/src/async_exec/hash_state.rs`
# Transaction-hash state machine: absent/pending/known/confirmed/failed.

20.
`crates/module-polymarket/src/async_exec/poller.rs`
# Bounded polling/backoff for unresolved transaction/trade state.

21.
`crates/module-polymarket/src/async_exec/backfill.rs`
# Resolve transaction hashes/trades after delayed matching.

22.
`crates/module-polymarket/src/async_exec/retry.rs`
# Safe retry rules without duplicating orders.

23.
`crates/module-polymarket/src/async_exec/reconciliation.rs`
# Async event -> existing reconciliation state.

24.
`crates/module-polymarket/src/async_exec/mod.rs`
# Async execution exports.

25.
`crates/module-polymarket/tests/async_trade_ids.rs`
# Response parsing/trade-ID tests.

26.
`crates/module-polymarket/tests/async_missing_hash.rs`
# Order accepted/matched but transaction hash temporarily absent.

27.
`crates/module-polymarket/tests/async_backfill.rs`
# Delayed hash/trade backfill and reconciliation tests.

========================================================
D. POLYMARKET TENANT EXECUTION
========================================================

28.
`crates/module-polymarket/src/tenant_execution/context.rs`
# Reuse the existing TenantExecutionContext and bind Polymarket account/order execution.

29.
`crates/module-polymarket/src/tenant_execution/guard.rs`
# Tenant/module/runtime/wallet/signer gate before Polymarket order creation.

30.
`crates/module-polymarket/src/tenant_execution/order.rs`
# Tenant-scoped order construction and submission.

31.
`crates/module-polymarket/src/tenant_execution/fill.rs`
# Tenant-scoped fill application.

32.
`crates/module-polymarket/src/tenant_execution/reconcile.rs`
# Tenant-scoped reconciliation and recovery.

33.
`crates/module-polymarket/src/tenant_execution/mod.rs`
# Tenant Polymarket execution exports.

34.
`crates/module-polymarket/tests/tenant_v3_execution.rs`
# Cross-tenant Polymarket execution tests.

========================================================
E. CUSTODY PROVIDER ABSTRACTION
========================================================

35.
`crates/core/src/custody/remote_signer.rs`
# Provider-neutral async remote signer contract; never expose private material.

36.
`crates/core/src/custody/signing_request.rs`
# Tenant-bound public signing request metadata.

37.
`crates/core/src/custody/signing_result.rs`
# Provider-neutral signature/result type.

38.
`crates/core/src/custody/provider_status.rs`
# Provider readiness/unavailable/degraded/healthy states.

39.
`crates/core/src/custody/provider_capabilities.rs`
# Explicit provider capability discovery.

40.
`crates/core/src/custody/mod.rs`
# Custody exports and existing-provider integration.

IMPORTANT:
If `remote_signer.rs` already exists, EXTEND the existing file rather than creating a duplicate.

========================================================
F. SERVER CUSTODY REGISTRY
========================================================

41.
`crates/server/src/custody/registry.rs`
# Runtime custody provider registry, tenant-aware, fail-closed, no silent local fallback.

42.
`crates/server/src/custody/resolver.rs`
# Resolve tenant binding -> provider -> signer reference.

43.
`crates/server/src/custody/readiness.rs`
# Provider readiness checks used before enabling trading.

44.
`crates/server/src/custody/audit_event.rs`
# Audit-safe custody events.

45.
`crates/server/src/custody/mod.rs`
# Custody server exports.

46.
`crates/server/tests/custody_registry.rs`
# Provider resolution/readiness tests.

47.
`crates/server/tests/custody_tenant_binding.rs`
# Tenant A cannot use Tenant B signer binding.

========================================================
G. VAULT
========================================================

48.
`crates/server/src/custody/vault/client.rs`
# Actual Vault client wrapper if the selected dependency supports the required signing operation.

49.
`crates/server/src/custody/vault/config.rs`
# Vault configuration using secret references/environment, never embedding credentials.

50.
`crates/server/src/custody/vault/signer.rs`
# Real Vault-backed signing implementation OR explicit unavailable provider path if required SDK is absent.

51.
`crates/server/src/custody/vault/health.rs`
# Vault connectivity/permission/readiness checks.

52.
`crates/server/src/custody/vault/mod.rs`
# Vault exports.

IMPORTANT:
NEVER return fake signatures.
If exact Vault cryptographic/signing API cannot be implemented with currently selected dependencies, return a real `Unavailable` implementation and identify the exact provider dependency still required.

========================================================
H. KMS
========================================================

53.
`crates/server/src/custody/kms/client.rs`
# Actual selected cloud KMS client wrapper.

54.
`crates/server/src/custody/kms/config.rs`
# KMS key/reference configuration without private material.

55.
`crates/server/src/custody/kms/signer.rs`
# Real KMS signing adapter.

56.
`crates/server/src/custody/kms/health.rs`
# KMS readiness/permission checks.

57.
`crates/server/src/custody/kms/mod.rs`
# KMS exports.

Same rule:
NO fake signing.

========================================================
I. BILLING AUTHORITATIVE STATE
========================================================

58.
`crates/server/src/billing/current_state.rs`
# Canonical authoritative commercial state read model.

59.
`crates/server/src/billing/state_transition.rs`
# Deterministic subscription/payment/invoice transition logic.

60.
`crates/server/src/billing/event_store.rs`
# Provider event persistence and idempotency.

61.
`crates/server/src/billing/payment_application.rs`
# Verified event -> transactionally applied payment state.

62.
`crates/server/src/billing/subscription_application.rs`
# Subscription lifecycle application.

63.
`crates/server/src/billing/invoice_application.rs`
# Invoice lifecycle application.

64.
`crates/server/src/billing/usage_meter.rs`
# Real tenant usage aggregation using actual trading data.

65.
`crates/server/src/billing/entitlement_snapshot.rs`
# Versioned entitlements consumed by runtime guards.

66.
`crates/server/src/billing/mod.rs`
# Billing exports.

67.
`crates/server/tests/billing_state_machine.rs`
# State transition tests.

68.
`crates/server/tests/billing_event_replay.rs`
# Duplicate-event/idempotency tests.

69.
`crates/server/tests/billing_usage_pg.rs`
# Live PostgreSQL tenant usage tests.

========================================================
J. CUSTOMER TRADING API
========================================================

70.
`crates/server/src/customer_api/trading_context.rs`
# Customer request -> existing TenantExecutionContext bridge.

71.
`crates/server/src/customer_api/orders.rs`
# Tenant order API.

72.
`crates/server/src/customer_api/positions.rs`
# Tenant positions/PnL API.

73.
`crates/server/src/customer_api/executions.rs`
# Tenant execution API.

74.
`crates/server/src/customer_api/sniper.rs`
# Tenant Sniper controls/status.

75.
`crates/server/src/customer_api/copy.rs`
# Tenant Copy Trading controls/status.

76.
`crates/server/src/customer_api/polymarket.rs`
# Tenant Polymarket controls/status.

77.
`crates/server/src/customer_api/telegram.rs`
# Tenant Telegram binding/status API.

78.
`crates/server/src/customer_api/billing.rs`
# Customer plan/usage/entitlement API using authoritative billing state.

79.
`crates/server/src/customer_api/mod.rs`
# Customer API exports.

80.
`crates/server/tests/customer_trading_api.rs`
# End-to-end API tenant isolation.

========================================================
K. CUSTOMER UI
========================================================

81.
`apps/control-plane/src/app/trading/page.tsx`
# Tenant trading dashboard using customer-only tenant APIs.

82.
`apps/control-plane/src/app/trading/orders/page.tsx`
# Tenant orders.

83.
`apps/control-plane/src/app/trading/positions/page.tsx`
# Tenant positions/PnL.

84.
`apps/control-plane/src/app/trading/executions/page.tsx`
# Execution lifecycle.

85.
`apps/control-plane/src/app/trading/sniper/page.tsx`
# Sniper module controls/status.

86.
`apps/control-plane/src/app/trading/copy/page.tsx`
# Copy configuration/status.

87.
`apps/control-plane/src/app/trading/polymarket/page.tsx`
# Polymarket V3 order/fill/status.

88.
`apps/control-plane/src/app/trading/telegram/page.tsx`
# Telegram tenant binding/status.

89.
`apps/control-plane/src/components/trading/RuntimeCard.tsx`
# Tenant runtime/fence/health status.

90.
`apps/control-plane/src/components/trading/OrderTable.tsx`
# Tenant order table with server pagination.

91.
`apps/control-plane/src/components/trading/PositionTable.tsx`
# Tenant positions.

92.
`apps/control-plane/src/components/trading/PnlCard.tsx`
# Tenant-only PnL summary.

93.
`apps/control-plane/src/components/trading/ModuleCards.tsx`
# Sniper/Copy/Polymarket runtime status.

94.
`apps/control-plane/src/lib/customer-trading-api.ts`
# Customer-only trading client, explicitly forbidden from using operator-global endpoints.

========================================================
L. BUYER PACKAGE PARITY
========================================================

95.
`scripts/compare-canonical-to-buyer-source.sh`
# Byte/content compare canonical source versus buyer-release/source, with explicit allowed exclusions.

96.
`scripts/rebuild-buyer-release.sh`
# Rebuild buyer package from canonical source.

97.
`scripts/update-release-manifest.sh`
# Regenerate current file counts/hashes/version/migration counts.

98.
`scripts/verify-release-integrity.sh`
# Combined source parity + manifest + contamination + version checks.

99.
`tests/release/buyer_source_parity.sh`
# Automated regression for buyer/source equality.

100.
`docs/CURRENT-BUYER-STATE.md`
# Single current buyer state document with current counts and current limitations.

========================================================
M. MARKETING / EVIDENCE
========================================================

101.
`docs/MARKETING-CLAIMS.md`
# Current safe/unsafe claims derived from actual code and evidence.

102.
`docs/LIVE-EVIDENCE-MATRIX.md`
# Maps each commercial claim to actual evidence status: code/test/live/funded/external-review.

103.
`docs/POLYMARKET-COMPATIBILITY-2026.md`
# Supported protocol versions and current unsupported areas.

104.
`docs/CUSTODY-STATUS-2026.md`
# Exact local/Vault/KMS/HSM implementation and readiness state.

105.
`docs/BILLING-STATUS-2026.md`
# Authoritative billing capabilities and limitations.

106.
`docs/CUSTOMER-SaaS-STATUS-2026.md`
# Actual customer-facing API/UI capabilities.

107.
`docs/BUYER-HANDOVER-STATUS-2026.md`
# Package, evidence, license, deployment and transfer readiness.

TARGET = 107 planned files.
The true number may be lower only when exact existing equivalents already exist.
NEVER create redundant duplicate sources.

========================================================
POLYMARKET IMPLEMENTATION RULES
========================================================

The current prompt MUST implement actual current protocol support.

Required:

1. V2 remains supported where current project already supports it.
2. V3 becomes explicit.
3. `position_id` semantics must be preserved exactly.
4. Async order response must handle:
   - order accepted;
   - transaction hash absent;
   - trade IDs present;
   - trade IDs absent;
   - later transaction resolution;
   - final success/failure.
5. Existing reconciliation must remain compatible.
6. No duplicate order on retry.
7. Tenant context must remain attached across:
   order submit
   →
   async polling
   →
   trade resolution
   →
   reconciliation
   →
   persistence.

Do not convert async state into immediate `confirmed`.

========================================================
POLYMARKET TESTS
========================================================

Must include:

- V2 regression
- V3 position order
- invalid V3 position
- wrong signature domain
- tradeIDs parsing
- missing transaction hash
- delayed transaction hash
- delayed trade match
- duplicate polling
- retry idempotency
- tenant A / tenant B isolation
- reconciliation isolation
- failure state preservation

========================================================
CUSTODY RULE
========================================================

Current source intentionally has only local signing.

Do not create fake remote custody to reduce the gap.

For each provider:

IF exact real SDK and API are already available:
implement it.

ELSE:
implement:
- provider boundary
- configuration
- readiness
- explicit unavailable status
- tenant binding
- audit
- integration test that proves refusal

Never:
- return fake signature
- sign locally when tenant selected remote provider
- silently fall back to local

That fallback would be a P0 security defect.

========================================================
BILLING RULE
========================================================

Remove any production-path synthetic behavior such as:

`starter`
`pro`
`42`
`2`
`manual`
`payment_state = null`
`usage = 0`

unless that value is actually read from authoritative DB state.

Production billing flow:

provider webhook
→ verify signature
→ event ID
→ event persistence
→ state transition
→ subscription/payment/invoice
→ entitlement version
→ runtime entitlement cache
→ module authorization

Any transaction failure must rollback atomically.

========================================================
CUSTOMER API RULE
========================================================

Client may NOT choose tenant arbitrarily.

Correct:

auth principal
→ membership
→ organization
→ runtime
→ entitlement
→ repository

Wrong:

request.organization_id
→ trust
→ DB

Customer API must be separate from operator-global API.

========================================================
CUSTOMER UI RULE
========================================================

No synthetic demo numbers.

Every page needs:

- loading
- empty
- error
- suspended tenant
- disabled module
- stale runtime
- unavailable custody
- entitlement denial

Customer pages MUST NOT call:
- global operator orders endpoint
- global operator positions endpoint
- global global-risk endpoint
- global reconciliation endpoint

unless the backend explicitly enforces tenant scope.

========================================================
BUYER PACKAGE RULE
========================================================

The canonical source must be the ONLY authority.

Process:

canonical tree
→ clean staging tree
→ buyer-release/source
→ manifest generation
→ checksums
→ contamination scan
→ parity scan
→ release archive

Verify exact:
- `.rs`
- `.sql`
- `.tsx`
- `.ts`
- `.json`
- `.toml`
- relevant `.md`
- scripts
- tests

The buyer package must NOT contain:
- `.env`
- credentials
- secrets
- `.pem`
- `.key`
- wallet JSONs
- dumps
- target
- node_modules
- .next
- .turbo
- .vercel
- machine/toolchain binaries

========================================================
MARKETING CLAIM VALIDATION
========================================================

Create a script that rejects unsupported claims such as:

"guaranteed"
"profit"
"risk-free"
"under 1 second guaranteed"
"fully audited"
"fully isolated"
"all Raydium"
"latest Polymarket V3"
"Vault/KMS/HSM included"
"self-service billing"
"institutional SLA"
"mainnet proven"

unless an explicit evidence file proves the claim.

Use evidence levels:
- CODE
- UNIT_TEST
- INTEGRATION_TEST
- LIVE_TEST
- FUNDED_TEST
- EXTERNAL_AUDIT

No marketing claim may use a higher evidence level than actually exists.

========================================================
IP / HANDOVER
========================================================

Add/extend documentation for:

- copyright holder
- repository ownership
- dependency license inventory
- SBOM
- deployment authority
- program ID
- treasury authority
- custody handover
- secret handover
- DNS/domain exclusion
- trademarks
- buyer acceptance
- source checksum

Do not fabricate owner names or transfer completion.

========================================================
REQUIRED VALIDATION
========================================================

Run:

cargo fmt --all -- --check

cargo check --workspace --all-targets

cargo clippy --workspace --all-targets -- -D warnings

Relevant:
cargo test -p module-polymarket
cargo test -p solana-kit
cargo test -p sniper-suite
cargo test -p bot-core
cargo test -p module-sniper
cargo test -p module-copy
cargo test -p module-telegram
cargo test -p saas-sdk

Run PostgreSQL-backed:
- Polymarket
- custody binding
- billing
- customer API
- buyer parity

Use `--test-threads=1` where current suite requires it.

Attempt:
cargo test --workspace

If memory/OOM prevents it:
report exact command and exact failure.
Never claim full workspace passed.

========================================================
POST-CODE SEARCH
========================================================

Search for:

`UnsupportedProvider`
`NOT_IMPLEMENTED`
`starter`
`pro`
`42`
`2`
`manual`
`payment_state`
`usage = 0`
`OrderV2`
`tradeIDs`
`position_id`
`organization_id`
`TenantExecutionContext`
`operator`
`buyer-release/source`
`placeholder`
`TODO`
`FIXME`
`stub`

Every match must be classified.

No unexplained match.

========================================================
NO-OMISSION OUTPUT CONTRACT
========================================================

For each phase:

1. COMPLETE new-file list.
2. COMPLETE modified-file list.
3. FULL content of every new file.
4. FULL content of every modified file.
5. FULL SQL for every migration.
6. FULL TS/TSX files.
7. FULL shell scripts.
8. Exact commands.
9. Exact test results.
10. Exact remaining gaps.

Never:
- snippet
- fragment
- diff-only
- ellipsis
- hidden imports
- hidden test code
- "same as above"
- "rest unchanged"
- "existing code"

========================================================
FINAL ACCEPTANCE CRITERIA
========================================================

PROMPT 5/10 is COMPLETE only if:

A. Polymarket V3 path exists or exact dependency blocker is documented.

B. Async trade-ID/transaction resolution exists.

C. V2 regression remains green.

D. Tenant context remains through async reconciliation.

E. Custody provider boundary is real and fail-closed.

F. No remote provider silently falls back to local.

G. Billing state is authoritative.

H. Provider event application is idempotent.

I. Usage is real DB-derived data.

J. Entitlements are consumed by runtime guards.

K. Customer APIs are tenant-safe.

L. Customer UI uses customer APIs, not operator-global routes.

M. Buyer package matches canonical source.

N. Release manifest matches tree.

O. Marketing claims are evidence-based.

P. Current documentation does not contradict current code.

Q. Existing Sniper/Copy behavior remains green.

R. Existing tenant execution boundary remains green.

S. No placeholder/stub/synthetic production behavior remains in the touched paths.

T. All modified files are supplied in full.

========================================================
FINAL RESPONSE FORMAT
========================================================

# PROMPT 5/10 RESULT

## 1. PRE-CODING FORENSIC MATRIX

finding
→ current file
→ current line
→ status
→ exact action

## 2. POLYMARKET V3

Show:
- V2
- V3
- position-backed orders
- async
- trade IDs
- backfill
- reconciliation

## 3. CUSTODY

Show:
- local
- Vault
- KMS
- HSM
- implemented
- unavailable
- exact reason

## 4. BILLING

Show:
- subscription
- payment
- invoice
- usage
- entitlement
- idempotency

## 5. CUSTOMER API

Show:
auth
→ tenant
→ entitlement
→ runtime
→ repository
→ response

## 6. CUSTOMER UI

List every page/component created or modified.

## 7. BUYER PACKAGE

Show:
canonical
→ buyer-release/source
→ manifest
→ checksums
→ verification

## 8. MARKETING

SAFE CLAIMS
UNSUPPORTED CLAIMS
EVIDENCE LEVEL

## 9. VALIDATION

Exact commands + exact result.

## 10. REMAINING GAP

Give separate percentages:
- exact 5-module engineering gap
- enterprise SaaS gap
- business/product gap
- evidence gap

## 11. FULL CODE

Every new/modified file in full.

## 12. NO-OMISSION DECLARATION

Explicitly state:
- no code skipped
- no placeholders
- no fake providers
- no fake billing
- no truncated files
- no omitted modified-file content