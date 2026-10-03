# On-call

**Status:** process defined; **rota, paging tool and alert rules are not
yet configured.** The gaps are listed at the end rather than implied away.

**Companion documents:** [`SLO-SLI.md`](SLO-SLI.md) ·
[`INCIDENT-RESPONSE-RUNBOOK.md`](INCIDENT-RESPONSE-RUNBOOK.md) ·
[`OPERATIONS-RUNBOOK.md`](OPERATIONS-RUNBOOK.md) ·
[`ROLLBACK-RUNBOOK.md`](ROLLBACK-RUNBOOK.md) ·
[`BACKUP-RESTORE.md`](BACKUP-RESTORE.md)

---

## The first thing to know

This system holds open positions and can sign transactions. The
instinctive engineering reflex — "restart it and see" — is wrong here.
A restart during an unreconciled state can re-enter a position or
double-execute an intent.

**Reconcile before you restart.** `RECOVERY_BLOCK_MODULES_ON_UNRESOLVED=true`
exists so the system refuses to trade while it is unsure; do not disable
it to make an alert go away.

## The two kill switches, and which to reach for

| Switch | Scope | How | When |
|---|---|---|---|
| **Tenant module pause** | one tenant, one module | `POST /api/tenant/{sniper\|copy\|polymarket}/controls` `{"action":"disable","reason":"…"}` | one customer's strategy is misbehaving |
| **Deployment kill switch** | everything | `KILL_SWITCH=true` (config/env), or `POST /api/kill` | the deployment is wrong, not one tenant |

Since migration **0036** the tenant pause is durable and shared across
replicas; before it, a pause only bound the replica that received it. If
you are operating a pre-0036 build, **scale to one replica first** or the
pause is a coin flip.

## Severity

| Sev | Definition | Response | Comms |
|---|---|---|---|
| **SEV-1** | Funds at risk or moving incorrectly; cross-tenant data exposure; custody key compromise suspected | page immediately, 24/7; engage security lead | customer comms within 1 h |
| **SEV-2** | Trading halted for all tenants; control plane down; database unavailable | page during extended hours; 30 min ack | status page |
| **SEV-3** | One module degraded; elevated error rate inside error budget; one tenant affected | next business day | ticket |
| **SEV-4** | Cosmetic, documentation, non-urgent toil | backlog | none |

**SEV-1 escalates immediately and is never downgraded by the person who
declared it.** If you are unsure between SEV-1 and SEV-2 on anything
touching funds or tenant boundaries, it is SEV-1.

## Alerts that must exist

None of these are configured yet. This is the specification for whoever
wires the monitoring stack, derived directly from
[`SLO-SLI.md`](SLO-SLI.md).

### Page (wake someone up)

| Alert | Condition | Why it pages | First action |
|---|---|---|---|
| `TenantIsolationViolation` | any log event `tenant_assert_failed` or a cross-tenant guard denial that should be impossible | S1 is a hard objective | SEV-1, see below |
| `CustodySignerMismatch` | signing attempted with a signer not resolved for the acting tenant | funds | SEV-1, revoke, `custody_rotation` |
| `ExecutionDuplicate` | the same idempotency key produced two executions | C2 is a hard objective | SEV-1, halt the module |
| `KillSwitchNotHonoured` | a tenant override exists in `tenant_module_controls` but a replica is still executing that module | S2 is a hard objective | SEV-1, scale to one replica |
| `ErrorBudgetFastBurn` | 2 % of the 30-day A1 budget consumed in 1 h | the deploy you just made is bad | `./scripts/rollback-release.sh production` |
| `ControlPlaneDown` | A2 probe failing for 3 consecutive minutes | SEV-2 | check nginx, then the app |
| `DatabaseUnavailable` | `bot_db_operations_total{outcome="error"}` > 50 % for 2 min | durable state is the system of record | SEV-2 |
| `FeedStale` | feed staleness > `WS_STALE_AFTER_MS` for 5 min while a module is enabled | trading on stale prices | pause the affected modules |
| `WorkerLeaseLapsed` | a `worker_claims` lane has been past `lease_until` for > 2 min | nobody is driving that tenant's pipeline | check replica health |
| `CertificateExpiring` | TLS certificate < 7 days remaining | the edge is about to go dark | `scripts/verify-tls-config.sh --live`, check certbot |

### Ticket (do not wake anyone)

`ErrorBudgetSlowBurn` (5 % in 6 h) · `LatencySLOBreach` (L1/L2) ·
`BackupMissed` (D1) · `RestoreDrillOverdue` (D2 > 90 d) ·
`DependencyAdvisory` (cargo-audit on `main`) ·
`BaseImageDigestMoved` (`pin-base-image-digests.sh --check`).

## First five minutes

1. **Acknowledge.** Silence stops a second pager from firing on the same
   thing.
2. **Classify.** Funds or tenant boundaries involved? → SEV-1.
3. **Stop the bleeding before diagnosing.**
   * one tenant → tenant module pause
   * all tenants → `KILL_SWITCH=true`
   * bad deploy → `./scripts/rollback-release.sh production`
4. **Snapshot evidence before mutating anything.** The audit trail is
   hash-chained and the journal is append-only; capture them now:
   ```bash
   docker compose logs --since 2h bot > /tmp/incident-$(date +%s).log
   curl -s localhost:8080/api/health | tee /tmp/incident-health.json
   ```
   `ops/incident_evidence.rs` and `ops/evidence_snapshot.rs` exist for
   this; see `INCIDENT-RESPONSE-RUNBOOK.md`.
5. **Open the incident channel and write a timeline as you go.** Not
   afterwards — afterwards is fiction.

## Triage: symptom → where to look

| Symptom | Look at | Likely cause |
|---|---|---|
| 503 `module_control_store_unavailable` | Postgres health, connection pool | the durable kill-switch store is unreachable; the API is correctly failing closed |
| 503 `rotation_store_unavailable` | Postgres | same, for custody rotations |
| 409 `rotation_already_in_flight` | `custody_rotations` for that profile | a previous rotation never reached a terminal state |
| WebSocket clients rejected `replay_detected` | `ws_replay_tokens`, clock skew between replicas | a client reusing a ticket, or clocks drifted |
| Orders accepted but never execute | `worker_claims`, replica logs | no replica holds the tenant's lane |
| Tenant sees another tenant's data | **STOP. SEV-1.** | isolation breach; preserve evidence, do not restart |
| Positions disagree with chain | recovery/reconcile logs | reconciliation lag or an RPC provider returning stale state |
| Deploy "succeeded", behaviour unchanged | `deploy/release/deployments.jsonl` | the running container is not the deployed digest — `deploy-release.sh` step 5 checks this |

## Rota

Not yet established. What it must specify before this document is
operational:

* primary and secondary, with a documented escalation path to a
  **security lead** (SEV-1 custody/isolation) and a **business owner**
  (customer comms);
* handoff ritual: open incidents, deploys in flight, error budget
  remaining, anything deliberately silenced;
* compensation and maximum consecutive weeks;
* a paging tool with an acknowledged SLA and a tested escalation chain
  — "we will use Slack" is not a paging tool.

## Post-incident

Within **5 business days** for SEV-1/2: a blameless review with a
timeline, contributing factors, what detected it (and what *should*
have), and dated action items with owners.

A SEV-1 review must answer two specific questions:

1. **Could an automated test have caught this?** If yes, the first
   action item is that test. The cross-tenant suites exist because this
   question was answered honestly before.
2. **Did an alert fire, and was it the right one?** A SEV-1 found by a
   customer is also a monitoring failure, and that gets its own action
   item.

## Honest gap list

1. No rota, no paging tool, no escalation chain.
2. None of the alerts above are implemented — there is no Prometheus,
   no Alertmanager, no external prober.
3. No status page (`status/statuspage.yml` does not exist).
4. `FeedStale` and `KillSwitchNotHonoured` need metrics that are not
   exported yet (see `SLO-SLI.md`, gaps 3 and 4).
5. No on-call has ever been exercised against this system; the triage
   table is derived from the code, not from incidents.

Until 1 and 2 are closed, this deployment is **not** operationally
on-call-ready, and should not be described as such to a buyer.
