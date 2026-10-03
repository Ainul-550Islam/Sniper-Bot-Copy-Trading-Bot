# Service level objectives and indicators

**Status:** defined, **not yet measured over a full window.**
**Owner:** @org/sre · **Review cadence:** quarterly

> **Read this first.** Every number below is a *target*, set from the
> product's requirements. None of them is a *measurement*: this
> deployment has not yet run a full 30-day window under production load,
> so there is no error budget history to report. Anything that presents
> these as achieved availability is a false claim. The "Evidence" column
> states exactly what exists today for each objective.

## Why SLOs at all

An SLO is the contract between "it is up" and "it is working". For a
system that holds positions, "the HTTP server returned 200" is not
up-ness: an API that answers quickly while the Solana feed is 40 seconds
stale is actively dangerous, because the user believes they are seeing
the market.

So the indicators below deliberately include **freshness and
correctness**, not only availability and latency.

## The indicators

| # | SLI (what is measured) | SLO (target) | Window | Evidence today |
|---|---|---|---|---|
| **A1** | Fraction of requests to `/api/saas/**` and `/api/tenant/**` returning a non-5xx status | **99.9 %** | 30 d rolling | metric `bot_http_requests_total{status}` exists; no production window recorded |
| **A2** | Fraction of successful `GET /api/health` probes | **99.95 %** | 30 d rolling | endpoint + Docker HEALTHCHECK exist; no external prober configured |
| **L1** | p95 latency of authenticated read endpoints | **< 300 ms** | 30 d rolling | histogram `bot_http_request_duration_ms` exists |
| **L2** | p99 latency of authenticated read endpoints | **< 1 s** | 30 d rolling | same histogram |
| **L3** | p95 database operation latency | **< 50 ms** | 30 d rolling | histogram `bot_db_operation_duration_ms` (emitted by `Database::timed`) |
| **F1** | Fraction of time the Solana WebSocket feed is fresher than `WS_STALE_AFTER_MS` (15 s) | **99.5 %** | 30 d rolling | staleness is enforced in `solana-kit`; **not yet exported as a gauge** |
| **F2** | Fraction of time no tenant worker lane has a lapsed lease | **99.9 %** | 30 d rolling | `worker_claims.lease_until` (migration 0032) is queryable; no alert wired |
| **C1** | Executions reconciled against on-chain truth within 5 min | **99.9 %** | 30 d rolling | recovery/reconcile loops exist (`RECOVERY_*` settings) |
| **C2** | Duplicate executions per 10 000 intents | **0** (hard) | always | idempotency keys + execution claims; cross-tenant suites in CI |
| **S1** | Cross-tenant data exposures | **0** (hard) | always | 9 cross-tenant PG suites in CI — the strongest evidence in this table |
| **S2** | Tenant kill-switch honoured by every replica within 5 s of the write | **100 %** (hard) | always | durable since migration 0036; `multi_replica_durable_state.rs` |
| **D1** | Successful nightly backup | **100 %** | 30 d | `ops/backup_verification.rs`, `docs/BACKUP-RESTORE.md`; **scheduling not yet automated** |
| **D2** | Restore drill completed and verified | **≥ 1 per quarter** | 90 d | `ops/restore_verification.rs`; no drill performed yet |

### Hard objectives

**C2, S1 and S2 have no error budget.** They are correctness
invariants, not availability targets. One cross-tenant exposure is an
incident regardless of how good the month was, and "99.9 % of tenant
pauses were honoured" is not a passing grade for a kill-switch.

## Error budget policy

For the *rate* objectives (A1, A2, L1–L3, F1, F2, C1, D1):

| Budget consumed in the window | Action |
|---|---|
| < 50 % | normal operation |
| ≥ 50 % | the next sprint prioritises reliability work over features |
| ≥ 75 % | feature deploys to production require SRE sign-off |
| ≥ 100 % | **feature freeze.** Only reliability fixes and security patches ship until the window recovers. |

A1 at 99.9 % over 30 days is **43 minutes** of budget. That is the
number to hold in your head: a single bad deploy can spend most of it.

## What is missing before these can be reported

Stated plainly, because a half-instrumented SLO is a worse artifact than
an honest gap list:

1. **No external prober.** A2 cannot be measured from inside the thing
   being measured. Needs a synthetic check from outside the deployment.
2. **No metrics retention.** `/metrics` is scraped by nothing today.
   Needs Prometheus (or equivalent) with ≥ 90-day retention, plus the
   `SNIPER_METRICS_ALLOW_CIDR` monitoring network from
   `deploy/nginx/sniper-suite.conf`.
3. **F1 has no gauge.** Feed staleness is *enforced* but not *exported*.
   One gauge (`bot_feed_staleness_ms`) closes it.
4. **No alert rules.** Objectives without alerts are documentation.
   `docs/ONCALL.md` lists the pages that need to exist.
5. **No burn-rate alerting.** Multi-window burn-rate alerts (fast: 2 % of
   budget in 1 h; slow: 5 % in 6 h) are the standard pairing and are not
   configured.
6. **D1/D2 are manual.** The verification code exists; nothing schedules
   it. See `docs/BACKUP-RESTORE.md`.

## Measurement definitions

So two people compute the same number:

* **Request counted** — any HTTP request that reaches the Axum router
  with a path under `/api/saas/` or `/api/tenant/`. Requests refused by
  nginx (rate limit, TLS failure) are *excluded* from A1 and tracked
  separately: an edge rate-limit is the system working, not failing.
* **Failure** — HTTP 500–599. 4xx is **not** a failure: a 401 or a 409 is
  the API correctly refusing. 429 from the application limiter is also
  not a failure.
* **Latency** — time from router entry to response head, excluding the
  client's body upload and excluding WebSocket connections (which are
  long-lived by design and would make any percentile meaningless).
* **Window** — 30 days rolling, evaluated continuously, not calendar
  months.
* **Multi-replica** — SLIs aggregate across replicas. A single replica
  being healthy while the other serves errors is a 50 % failure rate,
  not a pass.

## Not an SLA

These are internal objectives. No contractual service-level *agreement*
with financial remedies exists for this product, and none should be
offered on the strength of this document until items 1–6 above are
closed and at least two consecutive windows have been measured.
