# Principal Audit — Round 1 — 2026-10-05

Repository audited: `Ainul-550Islam/Sniper-Bot-Copy-Trading-Bot`

The user message did not contain an attached ZIP in the workspace. I cloned the
public repository URL supplied in the request and audited the resulting tree at
commit `HEAD` as available on 2026-10-05. No source code was executed with live
wallets, live billing credentials, live custody credentials, or funded trading
accounts.

## Scope and baseline

- 1,041 tracked files in the cloned tree.
- 622 Rust files, 72 TSX files, 20 TypeScript files, 38 SQL migrations, 50 shell scripts, and 158 Markdown documents.
- The repository is substantial and is not an empty scaffold, but the shipped application contains several demo-data paths that contradict its production-readiness claims.
- Rust validation could not be executed in this workspace because `cargo` and `rustc` are not installed. The frontend TypeScript compiler and targeted ESLint checks were executed.
- Frontend baseline: `npm run typecheck` passed after the Round 1 edits. Targeted ESLint passed with two pre-existing `react-hooks/set-state-in-effect` warnings.
- Frontend dependency audit: `npm audit` reports 5 high-severity transitive findings through `eslint-config-next` → `@next/eslint-plugin-next` → `fast-glob` → `micromatch` → `braces`. `npm audit fix` did not remediate them without a breaking downgrade.

## Critical findings

### P0 — false customer state from hardcoded or synthetic data

These are not merely presentation issues. They can cause an operator or customer to
believe that money-moving, alerting, analytics, infrastructure, or security
capabilities exist when the database or external service has not supplied them.

| Severity | Location | Evidence | Impact |
|---|---|---|---|
| P0 | `crates/server/src/saas/webhooks.rs:32-82` | Process-global `WEBHOOK_STORE`; first list call inserts a synthetic `https://api.yourdomain.com/...` record. | A restart loses webhooks, replicas disagree, and a customer sees a configured endpoint that was never created. |
| P0 | `crates/server/src/saas/webhooks.rs:212-244` | Test endpoint returns `success: true`, HTTP 200, and `latency_ms: 38` without locating the endpoint or sending an HTTP request. | The UI can report a successful delivery when no remote system received anything. |
| P0 | `crates/server/src/trading_data_plane/analytics.rs:41-79` | Metrics are selected from constants for `24h`, `7d`, `30d`, or a default branch; PnL series and execution quality are literal values. | Financial analytics are fabricated and are not derived from orders, trades, ledger, or accounting tables. |
| P0 | `apps/control-plane/src/app/analytics/page.tsx:13-29` | Executive metrics and module breakdown are hardcoded dollar values, trade counts, win rates, and Sharpe ratios. | Customers can be shown fictional performance even if the API is down. |
| P0 | `crates/server/src/trading_data_plane/backtests.rs:22-64,228` | In-memory store creates a sample record on first read and uses `BacktestService::simulate` to manufacture deterministic results. | Backtest history is lost on restart and simulated output is exposed as if it were persisted historical research. |
| P0 | `crates/server/src/trading_data_plane/strategies.rs:53-91` | `GET` creates two default strategy records in memory and returns them on demand. Create/update/archive do not persist to migration `0037`'s `strategies` table. | Strategy CRUD is not durable, multi-replica safe, or restart safe. |
| P0 | `crates/server/src/trading_data_plane/integrations.rs:30-88` | Five provider records and latencies are literal JSON values. | The UI reports connected providers and latencies without reading configuration or health evidence. |
| P0 | `crates/server/src/saas/status.rs:19-66` | Every platform component is returned as operational with fixed latencies and `active_incidents_count: 0`. | Customers cannot rely on the service-status page for incident truth. |
| P0 | `crates/server/src/trading_data_plane/onboarding.rs:51-111` | Process-global onboarding map defaults new tenants to active signer, funded wallet, and enabled module. | A new tenant can be told it has custody/funding/module prerequisites that were never verified. |

### P0 — frontend endpoint contract mismatch

`apps/control-plane/src/lib/api/strategy-api.ts` previously sent
`module_family` and `parameters`, while the Rust handler at
`crates/server/src/trading_data_plane/strategies.rs` deserializes `module` and
`config`. It also typed server responses as though they contained
`module_family` and `parameters`, while the Rust domain model serializes
`module` and `config_json`. The create/update requests therefore could fail
validation or render undefined fields.

Round 1 adds an explicit adapter in `strategy-api.ts` that translates the UI
model to and from the server wire contract. This does not close the separate
backend durability gap for strategies.

### P1 — API key UI used wrong server fields and a fake secret

The server returns `{"keys": [...]}` and each key uses `key_prefix` and `label`.
The original UI expected `items`, `prefix`, and `name`. On a create failure or
missing secret it substituted:

`snpr_live_mock_secret_84920194820194820`

That string was removed. The page now uses the shared typed API client, renders
an honest empty/error state, uses server-compatible role values, and only shows
the one-time secret returned by the server.

### P1 — webhook UI rendered a fake endpoint on any API failure

The original `settings/webhooks/page.tsx` replaced all failures with an
`api.acme-quant.com` sample record. Round 1 removes the fallback and surfaces a
retryable error state. The backend still requires the durable implementation
listed above.

### P1 — webhook credential and SSRF concerns

`crates/server/src/saas/webhooks.rs` currently stores webhook secrets in a
plain `secret` column, allows `http://localhost`, does not prevent redirect
based SSRF, and does not perform a real delivery. A production implementation
must use durable tenant-scoped rows, validate HTTPS URLs, disable redirects,
protect against private/link-local destinations and DNS rebinding, sign the
payload with HMAC, record every attempt in `webhook_deliveries`, and return the
actual remote result. The database migration has the endpoint and delivery
tables but the handlers do not use them.

### P1 — customer client masks backend failures with fabricated defaults

`apps/control-plane/src/lib/customer-trading-api.ts` contains fallback objects
for strategies, backtests, markets, sniper config, copy config, Polymarket
config, integrations, analytics, and onboarding (approximately lines
459-630). Those catches convert authorization errors, service outages, and
missing database state into empty or populated success values. The fallback
contract must be removed; pages should render `loading`, `empty`, `unavailable`,
`suspended`, or `error` based on the typed `ApiError`.

### P1 — stale dependency-security documentation

The repository documentation states that the frontend dependency graph has zero
vulnerabilities. A fresh `npm ci --ignore-scripts && npm audit` in this audit
returned five high-severity transitive findings involving `braces`,
`micromatch`, `fast-glob`, `@next/eslint-plugin-next`, and
`eslint-config-next`. This is not closed until a patched dependency path is
available or the lint dependency chain is redesigned and re-audited.

## Additional logic and security gaps

1. `strategies.rs` does not perform durable tenant-scoped CRUD despite migration `0037` creating the required table. It also constructs update records with a new timestamp and default module/mode rather than loading and locking the existing tenant row.
2. `backtests.rs` does not verify that `strategy_id` belongs to the authenticated organization before simulating. It silently substitutes current/default dates when RFC3339 parsing fails instead of returning a validation envelope.
3. `analytics.rs` accepts any timeframe and routes unknown values to an implicit all-time constant branch. Invalid input should be a 400 with a stable error and reason.
4. `integrations.rs`, `saas/status.rs`, and the integrations UI claim live or connected provider states without evidence level or health-registry linkage.
5. `apps/control-plane/src/app/settings/audit/page.tsx` still inserts four deterministic sample audit events when the API fails.
6. `apps/control-plane/src/lib/commercial.ts:295` catches security-summary failures and substitutes a fabricated all-clear object; an authorization or service error must remain visible.
7. The Polymarket strategy registry still has the documented unknown-strategy path in `crates/module-polymarket/src/strategy.rs`; unsupported configured names must be rejected before any order decision.
8. `crates/server/src/ops/audit_attestation.rs` still describes HMAC as a detached-signature placeholder. The public model needs to distinguish HMAC attestations from asymmetric/detached signatures rather than implying equivalence.
9. Configuration-held secret strings and custody adapter configuration need an explicit zeroization/lifecycle review. The repository's own open-items census records that `zeroize` is not currently used for non-test configuration secrets.
10. External live validation remains unexecuted: billing, remote custody, funded trading, production deployment, staking deployment identity, and independent audit must not be represented as complete by UI status pages.

## Round 1 code delivered in the workspace

The following complete files were updated:

1. `apps/control-plane/src/app/settings/api/page.tsx`
   - Removed hardcoded key rows and the fake secret fallback.
   - Corrected the list/create/revoke response contract.
   - Added honest empty and error states.
2. `apps/control-plane/src/app/settings/webhooks/page.tsx`
   - Removed fake webhook fallback.
   - Added typed error and retry behavior.
3. `apps/control-plane/src/lib/api/strategy-api.ts`
   - Added a typed adapter between UI names and Rust API names.
   - Corrected create/update payloads and response normalization.

## Recommended next batch

1. Replace `crates/server/src/saas/webhooks.rs` with a PostgreSQL-backed endpoint/delivery implementation and add tests for tenant isolation, HTTPS/SSRF policy, HMAC signatures, delivery failure, timeout, and replay-safe delivery ids.
2. Replace `crates/server/src/trading_data_plane/strategies.rs` with transactional CRUD over `strategies`, including tenant ownership, optimistic version checks, and `ApiErrorEnvelope` responses.
3. Replace `crates/server/src/trading_data_plane/backtests.rs` and `backtest_service.rs` with a durable queued job model; do not expose deterministic pseudo-results as historical performance.
4. Remove every fallback in `customer-trading-api.ts`, then convert the analytics, audit, integrations, onboarding, and status pages to real endpoint data with honest no-data states.
5. Remediate or isolate the five high frontend dependency findings and update the stale security documentation only after a fresh clean audit.

## Round 2 remediation update — 2026-10-05

The identified false-state paths were continued beyond the initial batch. The following changes are now present in the workspace:

- `crates/server/src/saas/webhooks.rs` now uses durable tenant-scoped endpoint and delivery rows, validates endpoint ownership and event input, signs test payloads with HMAC, and records actual delivery outcomes. Live network delivery and SSRF behavior still require integration validation.
- `crates/core/migrations/0039_webhook_hardening.sql` adds delivery uniqueness, endpoint status/indexing, and delivery timestamps needed by the durable webhook path.
- `crates/server/src/trading_data_plane/strategies.rs` now performs durable tenant-scoped strategy CRUD against migration 0037, validates module/mode/config input, and uses optimistic version checks for updates.
- `crates/server/src/trading_data_plane/backtests.rs` now persists queued tenant-owned jobs, verifies strategy ownership, and returns result metrics only from trusted worker-written JSON. The deleted `backtest_service.rs` is no longer used as a pseudo-result source.
- `crates/server/src/trading_data_plane/analytics.rs` now reads tenant accounting, positions, portfolio snapshots, executions, PnL series, and trade rows; unavailable statistical metrics are JSON null rather than zero.
- `crates/server/src/trading_data_plane/integrations.rs` and `crates/server/src/saas/status.rs` now expose only registered health-registry components and do not invent providers, latency, or check timestamps.
- `crates/server/src/trading_data_plane/onboarding.rs` now derives checklist state from tenant custody, wallet-balance, module-control, strategy, and paper-trade records.
- `crates/server/src/trading_data_plane/config_store.rs` provides durable tenant module configuration storage; module config handlers return null/empty evidence when no saved config exists rather than defaults.
- `crates/server/src/saas/reports.rs` and `crates/core/migrations/0040_durable_report_exports.sql` now generate and persist tenant-scoped CSV/JSON artifacts from authoritative rows, with strict date/type/format validation and tenant-scoped download checks.
- `crates/server/src/saas/audit_export.rs` now queries tenant-scoped durable audit rows, rejects malformed ranges, supports real JSON/CSV output, and recursively redacts secret-like fields.
- `crates/server/src/saas/custody_health.rs` now reports only durable custody profile/signer lifecycle evidence. It deliberately does not claim provider reachability and always keeps signing disallowed until a real health probe is recorded.
- `apps/control-plane/src/lib/customer-trading-api.ts` no longer masks backend failures with synthetic success objects. Nullable metrics/config contracts are represented explicitly.
- Analytics, integrations, onboarding, custody, reports, status, audit, backtest, and module configuration surfaces now render error, unavailable, empty, or null metric states honestly. The report UI no longer offers unsupported PDF generation.

Validation completed after the latest edits:

- `npm run typecheck` passed in `apps/control-plane`.
- `npm run build` passed in `apps/control-plane`; Next.js compiled and generated 38 routes.
- `git diff --check` passed.

Validation still unavailable or outstanding:

- Rust compilation, rustfmt, and Rust tests remain unavailable because `cargo`, `rustc`, and `rustfmt` are not installed in the workspace.
- PostgreSQL migrations and SQL column/type assumptions have not run against a live database.
- External webhook delivery, provider health probes, remote custody, billing, funded trading, worker processing, and production deployment remain unvalidated.
- Existing transitive frontend dependency audit findings remain unresolved.
- The project still contains local JSON error responses that should be reconciled with any future canonical `ApiErrorEnvelope` contract.
- Production readiness must not be claimed until Rust compilation and live integration validation are completed.

## Continuation remediation update — 2026-10-05

Additional backend hardening completed after the Round 2 update:

- Custody hydration now treats malformed required PostgreSQL columns as service-unavailable corruption instead of panicking or silently defaulting. Durable custody profile/signer inserts now distinguish duplicate resources from database failures, persist authenticated actor ownership, and reject signer creation/activation against suspended, revoked, or closed profiles. Durable update paths check affected-row counts so a concurrent disappearance cannot be reported as success. Profile/signer lifecycle, capability, and custody-rotation actions now also populate the dedicated tenant-scoped `custody_audit` table when PostgreSQL is configured. Migration 0041 extends the audit action check constraint for rotation events. Signer creation now persists the optional nullable `provider_ref` that hydration already reads.
- Custody rotation row decoding now refuses invalid provider, timestamp, nullable-field, failure-reason, and force-state values instead of substituting local-provider, current-time, or empty defaults.
- Both provider-neutral payment webhooks and the legacy subscription billing webhook now distinguish completed duplicates from in-flight claims, release unfinished durable claims when domain validation rejects an event, use an atomic process-local claim in no-database test mode, and fail closed when the bounded fallback cache is exhausted. Durable completion failures are surfaced as service-unavailable responses. The legacy path also retains compatibility reads for pre-0019 runtime markers without using them as the new concurrent claim mechanism.
- Outbound webhook SSRF filtering now rejects multicast, IPv4-mapped private/link-local addresses, and the complete IPv6 link-local range. Response bodies are streamed with a one-megabyte cap before a bounded digest is persisted; reqwest's stream feature is enabled in the workspace dependency declaration. Webhook endpoint secrets are now AES-256-GCM encrypted at rest using mandatory `WEBHOOK_SECRET_ENCRYPTION_KEY`; plaintext legacy rows fail closed until rotated through the new tenant-scoped secret-rotation endpoint.
- Billing reconciliation no longer invents a provider, event id, or event kind. The endpoint requires a PostgreSQL-backed, tenant-owned, completed `provider_events` row, persists decisions through the durable SaaS runtime-record adapter, and uses the process map only in the explicitly unavailable no-database compatibility path.
- Tenant module configuration reads now refuse stale-cache fallback when PostgreSQL is unavailable and report `effective_state: unknown`; writes audit accepted patches/removals transactionally in `tenant_config_audit`; malformed configuration documents and version overflow are refused rather than reset or silently versioned incorrectly.

Validation after these edits:

- `npm ci --ignore-scripts` completed successfully.
- `npm run typecheck` passed.
- `npm run build` passed and generated all 38 frontend routes.
- `npm audit --omit=optional --audit-level=high` still reports the same five high-severity transitive findings through `eslint-config-next`.
- `npm run lint` completed with zero errors and existing warning-level findings, including React effect warnings and unused imports.
- `git diff --check` passed.
- Rust compilation, rustfmt, Rust tests, PostgreSQL migration execution, and live provider/network validation remain unavailable or outstanding. Production readiness is not claimed.

## Continuation remediation update — control-plane contracts, security state, and deployment wiring — 2026-10-05

- Durable organization security policy now lives in `tenant_security_policies`; the security status, MFA enforcement, and CIDR allowlist handlers no longer use a process-global policy map.
- TOTP enrollment now generates a real random seed, stores only AES-256-GCM ciphertext under the stable `MFA_ENCRYPTION_KEY`, returns the one-time enrollment secret/otpauth URI, and verifies six-digit RFC 6238 codes with a bounded clock-skew window. The MFA key is documented in the environment templates and passed through `docker-compose.yml`; missing key configuration fails closed.
- Session authentication now checks durable MFA policy and requires a fresh session after enforcement changes. Login-code verification checks the durable device record, decrypts the seed, and records a replay-protected verification timestamp.
- Security credential rotation now revokes the authoritative `saas_runtime_records` session and API-key documents used by `PostgresSaasRepo`, not unrelated legacy tables.
- Invite revoke and token resend are durable, organization-scoped, permission-checked operations. Resend rotates the stored token hash and expiry and returns the plaintext token once with an explicit no-email-delivery disclosure; invite acceptance and external mail delivery remain unavailable.
- Risk and portfolio surfaces no longer return the former fabricated equity, drawdown, loss, or risk-rule constants. Portfolio data is loaded from tenant-scoped accounting, balances, positions, and snapshots; unavailable authoritative data returns service-unavailable rather than a synthetic number. Tenant risk state reads durable module controls and reports unavailable reference metrics as null.
- Module-control reads now fail closed on PostgreSQL errors instead of showing a stale replica cache. This protects the status surface, but the repository still requires end-to-end wiring of `TenantExecutionGateway` into every real order/trading-intent admission path before the tenant kill switch can be called an operational execution stop.
- Historical completion records that labelled unvalidated work “Full Production” now identify implementation inventory whose Rust, PostgreSQL, external-delivery, and funded-trading validation is pending.

Additional validation:

- Frontend typecheck, production build, lint, route/UI contract, fake-data, and whitespace gates passed after these changes. Lint remains zero errors with 53 warning-level findings.
- `cargo`, `rustc`, `rustfmt`, and `rustup` are not installed, so the Rust changes remain uncompiled and the generated OpenAPI artifact cannot be synchronized or checked.
- Docker is not installed, so `docker compose config` could not be run. PostgreSQL-backed MFA replay, credential rotation, invite, tenant-isolation, and kill-switch tests remain outstanding.
- No production-readiness claim is made.
