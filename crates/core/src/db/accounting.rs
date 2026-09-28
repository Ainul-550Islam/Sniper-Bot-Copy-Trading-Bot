//! Durable global risk / accounting repository (migration 0015, TASK 5).
//!
//! Backs the global ledger (`ledger_events` + `ledger_postings`), the
//! derived `global_positions` snapshots, the `global_risk_decisions`
//! journal, the runtime `kill_switches` (+ `kill_switch_events`) and the
//! `accounting_recon_findings`. Same conventions as `copy.rs` /
//! `polymarket.rs`: runtime-checked queries, guarded inserts, every call
//! through [`Database::timed`]; the record types are the plain data types of
//! [`crate::accounting`] / [`crate::global_risk`], so no caller needs `sqlx`.
//!
//! The event + postings insert runs in ONE transaction: either the event and
//! all of its balanced lines land, or nothing does — a half-written entry is
//! impossible. `INSERT … ON CONFLICT (event_id) DO NOTHING` is the durable
//! idempotency check; when it inserts nothing the postings are skipped and
//! the caller learns the fact was already booked.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::accounting::{
    Account, AccountingEvent, AccountingFinding, AccountingFindingKind, BookPosition, Entry,
    EntrySide, EventKind, EventSide, PositionKey, Posting, StoredEvent,
};
use crate::db::{Database, TimedDbError};
use crate::global_risk::{
    DecisionSnapshot, GlobalRejectReason, GlobalRiskDecision, GlobalRiskRequest, GlobalVerdict,
    KillScope, KillSwitchEvent, KillSwitchState,
};
use crate::models::{BotModule, ExecutionMode, Venue};

type RepoResult<T> = Result<T, TimedDbError>;

/// Repository over the 0015 tables.
pub struct AccountingRepo {
    db: Arc<Database>,
}

impl AccountingRepo {
    /// Repository over `db`.
    pub fn new(db: Arc<Database>) -> Self {
        AccountingRepo { db }
    }

    // ------------------------------------------------------------- events --

    /// Guarded insert of one event and its postings (one transaction).
    /// `Ok(true)` = new, `Ok(false)` = the event id was already journaled.
    pub async fn record_event(&self, stored: &StoredEvent, entry: &Entry) -> RepoResult<bool> {
        let e = &stored.event;
        let mut tx = self.db.pool().begin().await.map_err(TimedDbError::Error)?;
        let res = sqlx::query(
            r#"INSERT INTO ledger_events
                   (event_id, kind, module, venue, wallet, strategy, asset, quote_asset,
                    side, quantity, price, quote_amount, fee, mode, reference_id,
                    correlation_id, position_id, trade_id, counterparty_wallet, detail,
                    ts, recorded_at, replica_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                       $16, $17, $18, $19, $20, $21, $22, $23)
               ON CONFLICT (event_id) DO NOTHING"#,
        )
        .bind(&stored.event_id)
        .bind(e.kind.as_str())
        .bind(e.module.as_str())
        .bind(e.venue.as_str())
        .bind(&e.wallet)
        .bind(&e.strategy)
        .bind(&e.asset)
        .bind(&e.quote_asset)
        .bind(e.side.map(|s| s.as_str()))
        .bind(e.quantity)
        .bind(e.price)
        .bind(e.quote_amount)
        .bind(e.fee)
        .bind(e.mode.as_str())
        .bind(&e.reference_id)
        .bind(&e.correlation_id)
        .bind(&e.position_id)
        .bind(&e.trade_id)
        .bind(&e.counterparty_wallet)
        .bind(&e.detail)
        .bind(e.ts)
        .bind(stored.recorded_at)
        .bind(&stored.replica_id)
        .execute(&mut *tx)
        .await
        .map_err(TimedDbError::Error)?;
        if res.rows_affected() == 0 {
            tx.rollback().await.map_err(TimedDbError::Error)?;
            return Ok(false);
        }
        for p in &entry.postings {
            sqlx::query(
                r#"INSERT INTO ledger_postings
                       (event_id, seq, account, wallet, asset, side, amount, quantity, base_asset)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                   ON CONFLICT (event_id, seq) DO NOTHING"#,
            )
            .bind(&p.event_id)
            .bind(p.seq as i32)
            .bind(p.account.as_str())
            .bind(&p.wallet)
            .bind(&p.asset)
            .bind(p.side.as_str())
            .bind(p.amount)
            .bind(p.quantity)
            .bind(&p.base_asset)
            .execute(&mut *tx)
            .await
            .map_err(TimedDbError::Error)?;
        }
        tx.commit().await.map_err(TimedDbError::Error)?;
        Ok(true)
    }

    /// Every event, oldest first (fact time, then record time, then id).
    pub async fn load_events(&self) -> RepoResult<Vec<StoredEvent>> {
        let rows = self
            .db
            .timed(
                "ledger_events_load",
                sqlx::query(
                    r#"SELECT event_id, kind, module, venue, wallet, strategy, asset, quote_asset,
                              side, quantity, price, quote_amount, fee, mode, reference_id,
                              correlation_id, position_id, trade_id, counterparty_wallet,
                              detail, ts, recorded_at, replica_id
                       FROM ledger_events
                       ORDER BY ts ASC, recorded_at ASC, event_id ASC"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(stored_from_row).collect())
    }

    /// Postings of one event, in sequence.
    pub async fn postings(&self, event_id: &str) -> RepoResult<Vec<Posting>> {
        let rows = self
            .db
            .timed(
                "ledger_postings_load",
                sqlx::query(
                    r#"SELECT event_id, seq, account, wallet, asset, side, amount, quantity, base_asset
                       FROM ledger_postings WHERE event_id = $1 ORDER BY seq ASC"#,
                )
                .bind(event_id)
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(posting_from_row).collect())
    }

    /// Recent events, newest first (API).
    pub async fn recent_events(&self, limit: i64) -> RepoResult<Vec<StoredEvent>> {
        let rows = self
            .db
            .timed(
                "ledger_events_recent",
                sqlx::query(
                    r#"SELECT event_id, kind, module, venue, wallet, strategy, asset, quote_asset,
                              side, quantity, price, quote_amount, fee, mode, reference_id,
                              correlation_id, position_id, trade_id, counterparty_wallet,
                              detail, ts, recorded_at, replica_id
                       FROM ledger_events
                       ORDER BY recorded_at DESC, event_id DESC
                       LIMIT $1"#,
                )
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(stored_from_row).collect())
    }

    // ---------------------------------------------------------- positions --

    /// Upsert one aggregated-position snapshot.
    pub async fn upsert_position(&self, p: &BookPosition) -> RepoResult<()> {
        self.db
            .timed(
                "global_position_upsert",
                sqlx::query(
                    r#"INSERT INTO global_positions
                           (position_key, module, venue, wallet, strategy, asset, quote_asset, mode,
                            qty, cost_basis, realized, fees, bought_quote, sold_quote, bought_qty,
                            sold_qty, last_price, event_count, last_event_id, position_ids,
                            opened_at, updated_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                               $16, $17, $18, $19, $20, $21, $22)
                       ON CONFLICT (position_key) DO UPDATE SET
                           qty = EXCLUDED.qty,
                           cost_basis = EXCLUDED.cost_basis,
                           realized = EXCLUDED.realized,
                           fees = EXCLUDED.fees,
                           bought_quote = EXCLUDED.bought_quote,
                           sold_quote = EXCLUDED.sold_quote,
                           bought_qty = EXCLUDED.bought_qty,
                           sold_qty = EXCLUDED.sold_qty,
                           last_price = EXCLUDED.last_price,
                           event_count = EXCLUDED.event_count,
                           last_event_id = EXCLUDED.last_event_id,
                           position_ids = EXCLUDED.position_ids,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(p.key.as_string())
                .bind(p.key.module.as_str())
                .bind(p.key.venue.as_str())
                .bind(&p.key.wallet)
                .bind(&p.key.strategy)
                .bind(&p.key.asset)
                .bind(&p.key.quote_asset)
                .bind(p.key.mode.as_str())
                .bind(p.qty)
                .bind(p.cost_basis)
                .bind(p.realized)
                .bind(p.fees)
                .bind(p.bought_quote)
                .bind(p.sold_quote)
                .bind(p.bought_qty)
                .bind(p.sold_qty)
                .bind(p.last_price)
                .bind(p.event_count as i64)
                .bind(&p.last_event_id)
                .bind(&p.position_ids)
                .bind(p.opened_at)
                .bind(p.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Every snapshot (open and flat).
    pub async fn positions(&self) -> RepoResult<Vec<BookPosition>> {
        let rows = self
            .db
            .timed(
                "global_positions_load",
                sqlx::query(
                    r#"SELECT position_key, module, venue, wallet, strategy, asset, quote_asset, mode,
                              qty, cost_basis, realized, fees, bought_quote, sold_quote, bought_qty,
                              sold_qty, last_price, event_count, last_event_id, position_ids,
                              opened_at, updated_at
                       FROM global_positions ORDER BY position_key"#,
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(position_from_row).collect())
    }

    // ---------------------------------------------------------- decisions --

    /// Append one global risk decision.
    pub async fn record_decision(&self, d: &GlobalRiskDecision) -> RepoResult<()> {
        let snapshot = serde_json::to_value(&d.snapshot).unwrap_or(serde_json::Value::Null);
        self.db
            .timed(
                "global_risk_decision_record",
                sqlx::query(
                    r#"INSERT INTO global_risk_decisions
                           (decision_id, ts, module, venue, wallet, strategy, asset, quote_asset,
                            requested_quote, mode, verdict, reason, detail, snapshot, replica_id)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                       ON CONFLICT (decision_id) DO NOTHING"#,
                )
                .bind(&d.decision_id)
                .bind(d.ts)
                .bind(d.request.module.as_str())
                .bind(d.request.venue.as_str())
                .bind(&d.request.wallet)
                .bind(&d.request.strategy)
                .bind(&d.request.asset)
                .bind(&d.request.quote_asset)
                .bind(d.request.requested_quote)
                .bind(d.request.mode.as_str())
                .bind(match d.verdict {
                    GlobalVerdict::Accept => "accept",
                    GlobalVerdict::Reject => "reject",
                })
                .bind(d.reason.map(|r| r.as_str()))
                .bind(&d.detail)
                .bind(snapshot)
                .bind(&d.replica_id)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Recent decisions, newest first.
    pub async fn recent_decisions(&self, limit: i64) -> RepoResult<Vec<GlobalRiskDecision>> {
        let rows = self
            .db
            .timed(
                "global_risk_decisions_recent",
                sqlx::query(
                    r#"SELECT decision_id, ts, module, venue, wallet, strategy, asset, quote_asset,
                              requested_quote, mode, verdict, reason, detail, snapshot, replica_id
                       FROM global_risk_decisions ORDER BY ts DESC LIMIT $1"#,
                )
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(decision_from_row).collect())
    }

    // ------------------------------------------------------ kill switches --

    /// Upsert the current state of one runtime switch.
    pub async fn upsert_kill_switch(&self, s: &KillSwitchState) -> RepoResult<()> {
        self.db
            .timed(
                "kill_switch_upsert",
                sqlx::query(
                    r#"INSERT INTO kill_switches (scope, engaged, reason, actor, updated_at)
                       VALUES ($1, $2, $3, $4, $5)
                       ON CONFLICT (scope) DO UPDATE SET
                           engaged = EXCLUDED.engaged,
                           reason = EXCLUDED.reason,
                           actor = EXCLUDED.actor,
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(s.scope.as_string())
                .bind(s.engaged)
                .bind(&s.reason)
                .bind(&s.actor)
                .bind(s.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Append one switch change.
    pub async fn append_kill_switch_event(&self, e: &KillSwitchEvent) -> RepoResult<()> {
        self.db
            .timed(
                "kill_switch_event_append",
                sqlx::query(
                    r#"INSERT INTO kill_switch_events (scope, action, reason, actor, replica_id, ts)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                )
                .bind(e.scope.as_string())
                .bind(&e.action)
                .bind(&e.reason)
                .bind(&e.actor)
                .bind(&e.replica_id)
                .bind(e.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Every stored switch state.
    pub async fn load_kill_switches(&self) -> RepoResult<Vec<KillSwitchState>> {
        let rows = self
            .db
            .timed(
                "kill_switches_load",
                sqlx::query(
                    "SELECT scope, engaged, reason, actor, updated_at FROM kill_switches ORDER BY scope",
                )
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let scope = KillScope::parse(&r.try_get::<String, _>("scope").ok()?)?;
                Some(KillSwitchState {
                    scope,
                    configured: false,
                    engaged: r.try_get("engaged").unwrap_or(false),
                    reason: r.try_get("reason").unwrap_or_default(),
                    actor: r.try_get("actor").unwrap_or_default(),
                    updated_at: r.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
                })
            })
            .collect())
    }

    // ----------------------------------------------------------- findings --

    /// Append one accounting reconciliation finding.
    pub async fn append_finding(&self, f: &AccountingFinding) -> RepoResult<()> {
        self.db
            .timed(
                "accounting_finding_append",
                sqlx::query(
                    r#"INSERT INTO accounting_recon_findings
                           (finding_id, kind, module, venue, asset, position_id, event_id, trade_id,
                            order_id, expected, actual, detail, action, replica_id, ts)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
                )
                .bind(&f.finding_id)
                .bind(f.kind.as_str())
                .bind(f.module.map(|m| m.as_str()))
                .bind(f.venue.map(|v| v.as_str()))
                .bind(&f.asset)
                .bind(&f.position_id)
                .bind(&f.event_id)
                .bind(&f.trade_id)
                .bind(&f.order_id)
                .bind(f.expected)
                .bind(f.actual)
                .bind(&f.detail)
                .bind(&f.action)
                .bind(&f.replica_id)
                .bind(f.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Recent findings, newest first.
    pub async fn recent_findings(&self, limit: i64) -> RepoResult<Vec<AccountingFinding>> {
        let rows = self
            .db
            .timed(
                "accounting_findings_recent",
                sqlx::query(
                    r#"SELECT finding_id, kind, module, venue, asset, position_id, event_id, trade_id,
                              order_id, expected, actual, detail, action, replica_id, ts
                       FROM accounting_recon_findings ORDER BY ts DESC, id DESC LIMIT $1"#,
                )
                .bind(limit.clamp(1, 1000))
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rows.iter().filter_map(finding_from_row).collect())
    }
}

fn opt_string(r: &PgRow, col: &str) -> Option<String> {
    r.try_get::<Option<String>, _>(col).ok().flatten()
}

fn ts_or_now(r: &PgRow, col: &str) -> DateTime<Utc> {
    r.try_get(col).unwrap_or_else(|_| Utc::now())
}

fn stored_from_row(r: &PgRow) -> Option<StoredEvent> {
    let kind = EventKind::parse(&r.try_get::<String, _>("kind").ok()?)?;
    let module: BotModule = r.try_get::<String, _>("module").ok()?.parse().ok()?;
    let venue = Venue::parse(&r.try_get::<String, _>("venue").ok()?)?;
    let mode: ExecutionMode = r.try_get::<String, _>("mode").ok()?.parse().ok()?;
    let side = opt_string(r, "side").and_then(|s| EventSide::parse(&s));
    Some(StoredEvent {
        event_id: r.try_get("event_id").ok()?,
        event: AccountingEvent {
            kind,
            module,
            venue,
            wallet: r.try_get("wallet").unwrap_or_default(),
            strategy: r.try_get("strategy").unwrap_or_default(),
            asset: r.try_get("asset").unwrap_or_default(),
            quote_asset: r.try_get("quote_asset").unwrap_or_default(),
            side,
            quantity: r.try_get("quantity").unwrap_or_default(),
            price: r.try_get::<Option<f64>, _>("price").ok().flatten(),
            quote_amount: r.try_get("quote_amount").unwrap_or_default(),
            fee: r.try_get("fee").unwrap_or_default(),
            mode,
            reference_id: r.try_get("reference_id").unwrap_or_default(),
            correlation_id: opt_string(r, "correlation_id"),
            position_id: opt_string(r, "position_id"),
            trade_id: opt_string(r, "trade_id"),
            counterparty_wallet: opt_string(r, "counterparty_wallet"),
            ts: ts_or_now(r, "ts"),
            detail: r.try_get("detail").unwrap_or_default(),
        },
        recorded_at: ts_or_now(r, "recorded_at"),
        replica_id: r.try_get("replica_id").unwrap_or_default(),
    })
}

fn posting_from_row(r: &PgRow) -> Option<Posting> {
    Some(Posting {
        event_id: r.try_get("event_id").ok()?,
        seq: r.try_get::<i32, _>("seq").ok()?.max(0) as u32,
        account: Account::parse(&r.try_get::<String, _>("account").ok()?)?,
        wallet: r.try_get("wallet").unwrap_or_default(),
        asset: r.try_get("asset").unwrap_or_default(),
        side: EntrySide::parse(&r.try_get::<String, _>("side").ok()?)?,
        amount: r.try_get("amount").unwrap_or_default(),
        quantity: r.try_get("quantity").unwrap_or_default(),
        base_asset: opt_string(r, "base_asset"),
    })
}

fn position_from_row(r: &PgRow) -> Option<BookPosition> {
    let key = PositionKey {
        module: r.try_get::<String, _>("module").ok()?.parse().ok()?,
        venue: Venue::parse(&r.try_get::<String, _>("venue").ok()?)?,
        wallet: r.try_get("wallet").unwrap_or_default(),
        strategy: r.try_get("strategy").unwrap_or_default(),
        asset: r.try_get("asset").unwrap_or_default(),
        quote_asset: r.try_get("quote_asset").unwrap_or_default(),
        mode: r.try_get::<String, _>("mode").ok()?.parse().ok()?,
    };
    Some(BookPosition {
        key,
        qty: r.try_get("qty").unwrap_or_default(),
        cost_basis: r.try_get("cost_basis").unwrap_or_default(),
        realized: r.try_get("realized").unwrap_or_default(),
        fees: r.try_get("fees").unwrap_or_default(),
        bought_quote: r.try_get("bought_quote").unwrap_or_default(),
        sold_quote: r.try_get("sold_quote").unwrap_or_default(),
        bought_qty: r.try_get("bought_qty").unwrap_or_default(),
        sold_qty: r.try_get("sold_qty").unwrap_or_default(),
        last_price: r.try_get("last_price").unwrap_or_default(),
        opened_at: ts_or_now(r, "opened_at"),
        updated_at: ts_or_now(r, "updated_at"),
        event_count: r
            .try_get::<i64, _>("event_count")
            .unwrap_or_default()
            .max(0) as u64,
        last_event_id: r.try_get("last_event_id").unwrap_or_default(),
        position_ids: r
            .try_get::<Vec<String>, _>("position_ids")
            .unwrap_or_default(),
    })
}

fn decision_from_row(r: &PgRow) -> Option<GlobalRiskDecision> {
    let verdict = match r.try_get::<String, _>("verdict").ok()?.as_str() {
        "accept" => GlobalVerdict::Accept,
        "reject" => GlobalVerdict::Reject,
        _ => return None,
    };
    let snapshot: DecisionSnapshot = r
        .try_get::<serde_json::Value, _>("snapshot")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Some(GlobalRiskDecision {
        decision_id: r.try_get("decision_id").ok()?,
        ts: ts_or_now(r, "ts"),
        request: GlobalRiskRequest {
            module: r.try_get::<String, _>("module").ok()?.parse().ok()?,
            venue: Venue::parse(&r.try_get::<String, _>("venue").ok()?)?,
            wallet: r.try_get("wallet").unwrap_or_default(),
            strategy: r.try_get("strategy").unwrap_or_default(),
            asset: r.try_get("asset").unwrap_or_default(),
            quote_asset: r.try_get("quote_asset").unwrap_or_default(),
            requested_quote: r.try_get("requested_quote").unwrap_or_default(),
            mode: r.try_get::<String, _>("mode").ok()?.parse().ok()?,
        },
        verdict,
        reason: opt_string(r, "reason").and_then(|s| GlobalRejectReason::parse(&s)),
        detail: r.try_get("detail").unwrap_or_default(),
        snapshot,
        replica_id: r.try_get("replica_id").unwrap_or_default(),
    })
}

fn finding_from_row(r: &PgRow) -> Option<AccountingFinding> {
    Some(AccountingFinding {
        finding_id: r.try_get("finding_id").ok()?,
        kind: AccountingFindingKind::parse(&r.try_get::<String, _>("kind").ok()?)?,
        module: opt_string(r, "module").and_then(|m| m.parse().ok()),
        venue: opt_string(r, "venue").and_then(|v| Venue::parse(&v)),
        asset: opt_string(r, "asset"),
        position_id: opt_string(r, "position_id"),
        event_id: opt_string(r, "event_id"),
        trade_id: opt_string(r, "trade_id"),
        order_id: opt_string(r, "order_id"),
        expected: r.try_get("expected").unwrap_or_default(),
        actual: r.try_get("actual").unwrap_or_default(),
        detail: r.try_get("detail").unwrap_or_default(),
        action: r.try_get("action").unwrap_or_default(),
        replica_id: r.try_get("replica_id").unwrap_or_default(),
        ts: ts_or_now(r, "ts"),
    })
}
