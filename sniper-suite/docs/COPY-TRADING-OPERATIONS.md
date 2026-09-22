# Copy-Trading Operations Runbook

Operator guide for Module 2 (copy trading). Architecture and every knob:
[COPY-TRADING-ENGINE.md](COPY-TRADING-ENGINE.md). Crash points, restart
procedure and reconciliation findings:
[COPY-TRADING-RECOVERY.md](COPY-TRADING-RECOVERY.md). Generic incident,
journal and backup procedures: [OPERATIONS.md](OPERATIONS.md).

Copy trading mirrors somebody else's decisions with a delay. Treat every
number below as a bound on *how wrong things can go*, not as an expectation
of profit.

---

## 1. Enabling the module

1. Choose the feed in `[copy].feed`: `pumpportal` (websocket trade stream,
   lowest latency, needs `sniper.pumpportal_ws_url`), `logs_poll`
   (`getSignaturesForAddress` polling — `poll_interval_ms`,
   `poll_signature_limit`), or `transaction_subscribe` (Geyser / Helius
   websocket, needs `network.geyser_ws_url`).
2. Add leaders under `[[copy.wallets]]` (see §2). Start with
   `fixed_sol` sizing and a small amount; `paused = true` lets you observe a
   leader (events, counters, reconciliation) without mirroring anything.
3. Keep `execution.mode = "paper"` until the pipeline behaves as expected in
   the metrics and the audit trail (§4). Live requires
   `execution.mode = "live"` **and** `execution.allow_live_trading = true`.
4. Set the exposure controls in `[risk]` before going live:
   `copy_max_position_quote`, `copy_max_total_exposure_quote`,
   `copy_max_concurrent_positions`, `copy_max_leader_exposure_quote`,
   `copy_daily_loss_limit_quote`. Every `0` inherits the generic limit or
   disables the check — decide each one explicitly.
5. With Postgres configured the durable journal (`copy_leaders`,
   `copy_events`, `copy_links`) is used automatically; migration
   `0013_copy_trading` applies on connect (`auto_migrate`) or with your
   normal migration step.

Startup log lines to expect: `copy leader lifecycle` (one per configured
wallet, `event=followed`), `copy dedup re-seeded from journal` (when the
journal has recent events), `copy-trading bot running`.

---

## 2. Managing leaders

Membership, rules and the paused flag live in config and are **hot
reloaded**: edit `[[copy.wallets]]`, and on the next event or reconciliation
tick the registry syncs. Each change is one audit record
(`copy.leader.followed|rule_changed|paused|resumed|unfollowed`), one journal
row and one `copy_leader_events_total{event}` increment.

| Task | Change | Effect |
|---|---|---|
| follow a wallet | add a `[[copy.wallets]]` entry | `followed`; buys mirrored from the next event |
| stop mirroring but keep watching | `paused = true` | `paused`; buys refused `LEADER_PAUSED`, sells still mirrored, counters and reconciliation continue |
| resume | `paused = false` | `resumed` |
| change sizing / limits | edit the entry | `rule_changed`; applies to the next event |
| stop following | remove the entry | `unfollowed`; buys and sells refused `LEADER_REMOVED`; open positions stay under the sweeper's TP/SL; reconciliation flags `leader_removed_we_hold` |
| cap one leader | `max_exposure_sol`, `max_open_positions` | risk gate 1 (`LEADER_EXPOSURE`) |

Per-leader state and counters are in `copy_leaders` (status, `events_seen`,
`mirrored`, `rejected`, `last_event_at`, `last_slot`) and in the
`copy_leaders{status}` gauge; in-process, `CopyBot::leaders()` exposes the
registry snapshot (label, sizing summary, counters) for a future endpoint.

There is no API/Telegram command for leader lifecycle in this task.

---

## 3. Emergency controls

| Situation | Control | Scope |
|---|---|---|
| stop everything now | kill switch (`/api/kill`, Telegram, `risk.kill_switch`) | all modules, entries and exits refused by preflight; the sweeper's kill-switch rule flattens |
| stop new mirrors, keep exits | `risk.copy_emergency_disable = true` (`COPY_EMERGENCY_DISABLE=true`) | copy entries only (`STRATEGY_DISABLED`); TP/SL and mirrored exits keep running |
| stop one leader | `paused = true` on its entry | that leader's buys |
| stop copy trading | `copy.enabled = false` or module disable via API | the run loop skips events; the sweeper keeps managing open positions |
| copy losses | `risk.copy_daily_loss_limit_quote` | copy entries for the rest of the UTC day once realised copy PnL ≤ −limit |

Turning `copy_emergency_disable` on is logged as a config warning on every
load so it is never forgotten.

---

## 4. What to watch

**Metrics** (all `copy_*`, see the engine doc §11):

* `rate(copy_events_total[5m])` per `source` — the feed is alive. A drop to
  zero with the module enabled means the feed died (check the feed's own
  reconnect logs) or the leaders are idle.
* `copy_rejections_total{reason}` — the shape of your funnel.
  * `DUPLICATE_EVENT` high and `copy_events_total` high on two sources is
    normal (two feeds see the same trade).
  * `STALE_EVENT` climbing: the feed or your node is behind; look at
    `copy_detection_lag_ms` before loosening `max_staleness_secs`.
  * `LEADER_EXPOSURE`, `EXPOSURE_LIMIT`, `COPY_COOLDOWN`: your caps are
    binding — intended.
  * `EXECUTION_FAILED`, `BALANCE_UNAVAILABLE`, `OWNERSHIP_UNAVAILABLE`:
    infrastructure. These are `FAILED`, not `REJECTED`, and each appears in
    the audit trail.
  * `OUT_OF_ORDER` with `strict_ordering` on: the feed replays or pages
    backwards; consider advisory mode unless late fills are unacceptable.
* `copy_ordering_total{verdict="gap"}` — a numbered source skipped
  deliveries. Reconciliation covers missed exits; missed entries are simply
  not mirrored.
* `copy_total_latency_ms` / `copy_<stage>_latency_ms` — where time goes.
  `copy_risk_latency_ms` or `copy_claimed_latency_ms` growing points at the
  ownership store / database.
* `copy_leader_exposure_sol_milli{leader}` — open exposure per leader,
  refreshed every reconciliation pass.
* `copy_recon_findings_total{kind}` — non-zero `leader_exited_we_hold` means
  you are holding something the leader left (see §6).
* `copy_journal_errors_total{op}` — the durable journal is failing; trades
  are unaffected (best effort) but recovery after a restart degrades.
* `bot_symbol_gated_entries_total{module="copy"}` — entries refused while a
  mint has unresolved reconciliation claims.

**Audit trail** (`actor = "copy"`, greppable `key=value` outcome lines):

| Action | Meaning |
|---|---|
| `copy.entry.filled` / `copy.entry.ambiguous` | a mirror opened (or may have); line carries `intent=`, `position=`, `requested_sol=`, `sized_sol=`, `total_ms=` |
| `copy.entry.rejected` / `copy.entry.failed` | a non-routine refusal; `reason=`, `at=` (stage), `detail=` |
| `copy.exit.exit_mirrored` / `copy.exit.failed` | a leader exit mirrored / failed |
| `copy.leader.<event>` | registry transition with `reason=` |
| `copy.recon.<kind>` | reconciliation finding with `action=` |
| `copy.recovery.<action>` | restart recovery action |

Routine refusals (duplicates, unknown leaders, `BELOW_LEADER_MIN`,
cooldowns, `OWNERSHIP_LOST`, `REPLAY_ONLY`) are **not** audited — use the
metrics and the `copy_events` table for those.

**Durable journal** (Postgres):

```sql
-- funnel for one leader over the last hour
select stage, reject_reason, count(*) from copy_events
 where leader = $1 and observed_at > now() - interval '1 hour'
 group by 1, 2 order by 3 desc;

-- what we hold from whom
select l.leader, l.mint, l.position_id, l.follower_qty, l.opened_at
  from copy_links l where l.status = 'open' order by l.opened_at;

-- one leader trade end to end
select e.stage, e.reject_reason, e.intent_id, e.position_id,
       x.state as ledger_state, x.signature
  from copy_events e left join execution_lifecycle x on x.intent_id = e.intent_id
 where e.signature = $1;
```

---

## 5. Tuning

| Symptom | Knob | Note |
|---|---|---|
| mirrors too large / small | wallet `fixed_sol` or `fraction_of_their_size` + `max_sol`; global `max_sol_per_trade`, `max_balance_fraction`; `risk.copy_max_position_quote` | the audit line shows `requested_sol` (sizing) vs `sized_sol` (risk) |
| many tiny mirrors | `min_mirror_sol` (`DUST_SIZE`) or wallet `min_sol` (`BELOW_LEADER_MIN`) | |
| late fills | `max_staleness_secs` (per wallet), `max_event_age_secs` (global, chain time) | measure `copy_detection_lag_ms` first |
| too many open mirrors | `risk.copy_max_concurrent_positions`, wallet `max_open_positions` | |
| one leader dominates | `risk.copy_max_leader_exposure_quote`, wallet `max_exposure_sol` | |
| repeated failures on a mint | `risk.copy_failed_entry_cooldown_secs` | shared failed-entry map with the sniper |
| too many in-flight entries | `risk.copy_max_pending_executions` | exits never count |
| reconciliation noise | `reconcile_interval_secs` | `0` disables; findings are idempotent |

Config hot reload applies to all of the above without a restart.

---

## 6. Handling reconciliation findings

`CopyBot::reconcile_once` runs every `reconcile_interval_secs` and at each
pass publishes `copy.recon.<kind>` with the suggested action.

| Finding | What happened | What the engine does | What you do |
|---|---|---|---|
| `leader_exited_we_hold` | the leader sold a mint you still hold (observed while paused, while `mirror_exits` was off, or while `buys_only`) | `Flag`; with `reconcile_auto_exit = true` **and** `mirror_exits = true` it sells through the normal exit path (`copy.recon.mirror_exit_done`) | decide: enable auto exit, sell manually, or hold under TP/SL |
| `leader_removed_we_hold` | you unfollowed a leader while holding its mint | `Flag` | the sweeper keeps TP/SL; sell manually if desired |
| `link_without_position` | the sweeper / an operator closed the position | closes the link (`closed` / `orphaned`) | nothing |
| `quantity_mismatch` | a partial exit changed our quantity | refreshes the link | nothing |
| `orphan_position` | a copy position without link or leader attribution | `Flag` | investigate how it was created |
| `ambiguous_entry` | the entry's ledger record is still live | `Flag` | wait for the execution reconciler (RECONCILIATION.md); never resubmit |

Reconciliation never sells on a flag; the only selling path is
`reconcile_auto_exit`, which is a config warning on every load.

---

## 7. Failure modes and expected behaviour

| Failure | Behaviour |
|---|---|
| feed disconnects | the feed's own reconnect/backoff; duplicates on reconnect are `DUPLICATE_EVENT`; a replayed backlog older than `max_event_age_secs` is `STALE_EVENT` |
| RPC unhealthy | `BALANCE_UNAVAILABLE` (live) or `EXECUTION_FAILED` at curve load; the event is decided (not retried); the mint gets the failed-entry cooldown |
| ownership store down | `OWNERSHIP_UNAVAILABLE` — fail closed, nothing sent |
| Postgres down | journal writes fail (`copy_journal_errors_total`), trading continues; `load_leaders` / `events_since` answer `None` → recovery logs that it could not re-seed |
| process crash | see COPY-TRADING-RECOVERY.md; nothing is ever resubmitted |
| leader front-runs / dumps | not a system failure; your caps (`max_exposure_sol`, `copy_daily_loss_limit_quote`) bound the damage; the sweeper's stop loss applies |

---

## 8. Daily checks

1. `copy_leaders{status="active"}` matches the config; no unexpected
   `paused` / `removed`.
2. `copy_events_total` is moving for every active leader; `last_event_at`
   in `copy_leaders` is recent for leaders you expect to be trading.
3. `copy_rejections_total{reason=~"EXECUTION_FAILED|BALANCE_UNAVAILABLE|OWNERSHIP_UNAVAILABLE"}`
   is flat.
4. `copy_recon_findings_total{kind="leader_exited_we_hold"}` — act on any
   increase (§6).
5. `copy_journal_errors_total` is flat.
6. Open links equal open copy positions:
   `select count(*) from copy_links where status = 'open'` vs the position book.
