# Final incident / recovery runbook

17 scenarios, each with: DETECTION → IMMEDIATE ACTION → SAFE STATE →
RECOVERY → VERIFICATION → POST-INCIDENT EVIDENCE. Grounded strictly in
implemented behavior (degradation matrix in `docs/OPERATIONS.md`, state
machine in `docs/RECONCILIATION.md`, executed tests). Severity hints assume
live mode; in paper/simulate most scenarios are observe-and-fix.

General rule for ALL incidents: never edit the audit table or journal files;
preserve them as evidence. Kill switch (`/api/kill`, Telegram `/kill`)
stops NEW entries immediately while in-flight orders continue to
confirmation/reconciliation — it is the safe default when in doubt.

## 1. RPC outage (Solana)

* DETECTION: execution/read error-rate metrics spike; logs show typed RPC
  errors; failover rotation messages.
* IMMEDIATE ACTION: none if failover list has healthy endpoints (client
  rotates automatically — failover tests executed). If ALL endpoints fail:
  kill switch ON (new entries need chain reads; sizing is fail-closed).
* SAFE STATE: modules idle; open orders continue to be tracked; simulate/
  live sends refused while RPC is down (never blind-broadcast).
* RECOVERY: restore/replace endpoints in config failover list (§RPC
  rotation, ops handover §12); kill switch OFF after reads verify.
* VERIFICATION: `GET /ready` RPC component healthy; one simulate-mode cycle
  clean; latency metrics normal.
* EVIDENCE: error-rate window in metrics; log excerpts; config diff.

## 2. Redis outage

* DETECTION: `/ready` Redis component degraded; dedup/claim fast-path
  warnings.
* IMMEDIATE ACTION: none for data safety — Redis is accelerator-only; PG
  fallbacks engage per the degradation matrix. For a 2-replica setup,
  consider pausing new entries (claim contention falls back to PG advisory
  paths, slower).
* SAFE STATE: trading continues degraded (documented matrix) or paused by
  operator choice; NO financial state is lost (nothing lives solely in
  Redis).
* RECOVERY: restart/replace Redis; app reconnects; claim/lease state
  re-derives from PG (authoritative).
* VERIFICATION: `/ready` green; distributed claims behave (watch duplicate-
  mirror counters at zero).
* EVIDENCE: degradation-window logs; matrix row references.

## 3. PostgreSQL outage

* DETECTION: `/ready` DB component down; DB error logs; this is the most
  severe outage — durable state is unreachable.
* IMMEDIATE ACTION: kill switch ON; let in-flight orders reach
  confirmation (signatures are journalled locally); do NOT restart-loop the
  app.
* SAFE STATE: no new intents (DB writes fail loudly — no silent in-memory
  continuation of money state); journal buffers recent intent history on
  disk.
* RECOVERY: restore PG (service fix or backup restore per
  `docs/BACKUP-RESTORE.md`); start app → recovery pass replays/reconciles
  journal + `Unknown` orders.
* VERIFICATION: `_sqlx_migrations` 11/11; `/api/audit/verify` → `intact`;
  recon queue drains; balances reconcile with chain.
* EVIDENCE: outage window, journal files, recovery logs, audit-verify
  output.

## 4. Telegram outage (Bot API unreachable)

* DETECTION: alert-send errors (token-redacted); command polling failures.
* IMMEDIATE ACTION: none — Telegram is a control surface only; trading does
  not depend on it. Use the HTTP API (`/api/*`) for control meanwhile.
* SAFE STATE: full trading continues; remote commands unavailable.
* RECOVERY: Telegram-side (or token revoked → rotate per ops §11).
* VERIFICATION: `/status` command answers; alert delivery resumes.
* EVIDENCE: redacted error logs; note that NO error string contains the
  token (regression-tested).

## 5. WebSocket disconnect (PumpPortal / Geyser / CLOB)

* DETECTION: feed reconnect logs; detection latency metrics rise; poll
  fallback engages automatically (documented feed behavior).
* IMMEDIATE ACTION: none (auto-reconnect + poll fallback); if the outage is
  long and the module is latency-critical (sniper), consider pausing the
  module.
* SAFE STATE: poll fallback or module paused; no orders from stale feeds
  (launch freshness bounds reject old observations).
* RECOVERY: reconnect happens automatically; verify feed lag metrics drop.
* VERIFICATION: feed event counters resume; one detected launch flows
  through screening.
* EVIDENCE: reconnect log window; fallback-mode metrics.

## 6. Duplicated event (feed delivers the same launch/trade twice)

* DETECTION: duplicate-event counters; dedup hit metrics.
* IMMEDIATE ACTION: none — dedup is automatic: Redis accelerator +
  PG-authoritative idempotency keys (money-critical layer).
* SAFE STATE: second copy is dropped before intent creation (tested: dedup
  suites + two-replica mirror).
* RECOVERY: n/a.
* VERIFICATION: orders table shows exactly one intent per event id.
* EVIDENCE: dedup hit logs; DB query showing single intent.

## 7. Duplicate order attempt (restart race / replica race)

* DETECTION: OMS idempotency-key rejection logs.
* IMMEDIATE ACTION: none — insert with the same idempotency key is
  rejected by PG constraint, not by timing luck.
* SAFE STATE: one order exists; the loser of the race logs the rejection.
* RECOVERY: n/a. VERIFICATION: `GET /api/orders` count vs intent journal.
* EVIDENCE: rejection log lines; DB state.

## 8. Stale balance (chain read older than freshness bound)

* DETECTION: typed `BalanceUnavailable` / stale-snapshot rejections in logs
  (Polymarket collateral snapshot bound; Solana sizing refuses cached reads
  outside paper mode).
* IMMEDIATE ACTION: none — the order is REJECTED (fail-closed by design;
  reject-over-fallback is regression-tested).
* SAFE STATE: no order placed on unverified funds.
* RECOVERY: fix the RPC/read path; rejections stop.
* VERIFICATION: next scan sizes against a fresh verified read.
* EVIDENCE: rejection log lines with typed error names.

## 9. Insufficient funding

* DETECTION: typed `InsufficientFunding` (Polymarket) / risk rejection
  `available_quote` (Solana) at sizing time.
* IMMEDIATE ACTION: none automatically — this is a pre-trade rejection,
  not an incident, unless it signals a wallet-drift (unexpected balance →
  investigate withdrawals).
* SAFE STATE: order rejected before submission.
* RECOVERY: fund the wallet / adjust per-trade sizing config.
* VERIFICATION: balance reads match expectations (`/api/status`).
* EVIDENCE: rejection logs; balance snapshots.

## 10. Transaction confirmation timeout

* DETECTION: confirmation-pending metrics age; order state `Unknown` /
  `SendUnknown` classification.
* IMMEDIATE ACTION: none manually — the outcome-unknown path is explicit:
  NEVER assumed failed, NEVER auto-rebroadcast (double-spend risk); it
  enters the reconciliation queue.
* SAFE STATE: order parked as `Unknown`; recon sweep polls the chain by
  signature.
* RECOVERY: automatic on signature discovery (landed → decode + persist;
  provably terminal failure → re-queue per policy).
* VERIFICATION: recon queue item resolves; `GET /api/orders` state
  converges; chain shows the signature or its absence past the block
  horizon.
* EVIDENCE: signature, recon item lifecycle, attribution row
  (migration 0006 table).

## 11. Partial execution (swap filled partially / one leg of a multi-step)

* DETECTION: decoded confirmed-tx facts disagree with the intended fill
  (amounts); recon flags divergence.
* IMMEDIATE ACTION: kill switch if the venue is misbehaving systematically;
  otherwise let reconciliation record the true on-chain state.
* SAFE STATE: position book reflects CONFIRMED on-chain facts (decode of
  the confirmed transaction is the fill truth in live mode — not the plan).
* RECOVERY: operator reviews the position; exit/adjust via normal flows.
* VERIFICATION: positions vs chain balances match.
* EVIDENCE: confirmed signature, decoded facts, position rows before/after.

## 12. Reconciliation mismatch (book vs chain)

* DETECTION: recon mismatch queue items; balance drift alerts you wire.
* IMMEDIATE ACTION: stop new entries for the affected module (kill switch
  if broad); do NOT hand-edit the DB.
* SAFE STATE: trading paused for the affected scope; chain is the source
  of truth.
* RECOVERY: work the queue per `docs/OPERATIONS.md` §"did my order actually
  land?" — resolve each item with signature evidence; only then resume.
* VERIFICATION: queue empty; `/api/audit/verify` intact; balances match.
* EVIDENCE: queue item history (claim-event lineage table), resolutions.

## 13. Application crash (panic / OOM-kill)

* DETECTION: process gone; supervisor alert; last logs before exit.
* IMMEDIATE ACTION: restart the app (crash-safe by design).
* SAFE STATE: PG + journal hold all durable state; in-flight signatures
  recoverable.
* RECOVERY: boot recovery pass: reload positions, re-register unfinished
  orders as `Unknown`, startup reconcile gate holds modules until resolved.
* VERIFICATION: recovery log sequence; recon queue processes; `/ready`
  green; no duplicate orders (idempotency).
* EVIDENCE: crash logs, recovery logs, journal tail.

## 14. Machine restart (host reboot)

* DETECTION: n/a (planned or power event).
* IMMEDIATE ACTION: on boot: PG + Redis first, then the app (compose
  dependency order encodes this).
* SAFE STATE: identical to crash recovery (#13).
* RECOVERY: startup recovery runs automatically; if the journal has
  unflushed tail lines, corrupt-line recovery skips/parks them loudly
  (tested) — review parked lines.
* VERIFICATION: as #13 + journal integrity messages in logs.
* EVIDENCE: boot logs; journal scan output.

## 15. Kill switch activation (by operator or automation)

* DETECTION: `/api/status` `kill_switch: true`; audit log entry.
* IMMEDIATE ACTION: understand WHY it was set before clearing; new entries
  are already stopped; in-flight orders continue to confirmation/recon.
* SAFE STATE: no new entries; exits and reconciliation still operate.
* RECOVERY: fix the trigger condition → `/api/resume` (or Telegram
  `/resume`, owner) → verify `kill_switch: false`.
* VERIFICATION: one paper/simulate cycle clean before re-enabling live.
* EVIDENCE: audit-chain entry for the flag change (append-only).

## 16. Database corruption (PG-level)

* DETECTION: PG errors (checksum failures, crash-recovery loops),
  audit-verify returning tamper/corruption results.
* IMMEDIATE ACTION: kill switch ON; STOP the app; do not run migrations
  against a corrupt DB.
* SAFE STATE: app down; corrupt DB preserved for forensics (copy it aside).
* RECOVERY: restore latest verified backup into a fresh cluster
  (`docs/BACKUP-RESTORE.md`); replay intent journal for the gap window
  (journal is on the app disk, not in PG); run recovery pass.
* VERIFICATION: `_sqlx_migrations` 11/11; `/api/audit/verify` → `intact`;
  recon queue drains; chain-vs-book balances match for the gap window.
* EVIDENCE: backup dump hash used, gap-window journal, reconcile results,
  audit-verify output before/after.

## 17. Deployment rollback (bad release)

* DETECTION: post-deploy health/readiness failures, error-rate spike,
  behavior regression.
* IMMEDIATE ACTION: redeploy the previous artifact (ops handover §13).
  Migrations are additive/forward-only — the old binary runs on the newer
  schema. If a migration itself is the problem: restore from backup (§16
  path) — the reason backup cadence + restore drills are mandatory.
* SAFE STATE: previous version serving; kill switch ON until verified.
* RECOVERY: verify health/readiness → paper cycle → resume.
* VERIFICATION: version endpoint/build info shows the rolled-back version;
  gate re-run if time permits (`release-check.sh`).
* EVIDENCE: deploy records (artifact hashes both versions), health windows,
  incident timeline.

---

## Universal post-incident checklist

1. Timeline written (detection → resolution, UTC).
2. Logs + journal + metrics window archived (read-only copy).
3. Audit chain exported/verified (`/api/audit/verify`) — never edited.
4. Signatures of every in-flight transaction recorded with final states.
5. Root cause classified: config / infra / code / external venue / feed.
6. If code: regression test added BEFORE the fix ships (project policy —
   the hardening pass itself followed this: defects D1/D2 fixed with
   executed e2e proof).
7. Runbook updated if the scenario behaved differently than documented.
