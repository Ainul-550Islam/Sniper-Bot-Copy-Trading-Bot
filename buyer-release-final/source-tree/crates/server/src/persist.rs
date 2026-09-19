//! Persistence pump (BUILD PLAN §4-iii/§7).
//!
//! ONE event-bus consumer materialises durable state into PostgreSQL:
//! OMS orders, on-chain transactions, trades, positions, risk events and
//! system events. Centralising this here means the trading modules keep
//! their publish-and-forget event contract while every financial row still
//! lands in the database exactly once:
//!
//! * idempotency keys are deterministic hashes of the event identity, so a
//!   replayed journal or a restarted pump cannot duplicate orders;
//! * every write is an upsert/guarded insert (see `bot_core::db::repo`);
//! * failures are metered + logged, never fatal — the pump keeps draining
//!   the bus so one bad row cannot wedge persistence.
//!
//! The pump also owns the restart-restore path: [`restore`] loads open
//! positions and unfinished orders back into memory before trading resumes.

use std::sync::Arc;
use std::time::Duration;

use bot_core::auth::sha256_hex;
use bot_core::db::repo::{
    OrderRepo, PositionRepo, ReconRepo, RiskEventRepo, SystemEventRepo, TradeRepo, TransactionRepo,
};
use bot_core::db::Database;
use bot_core::events::AppEvent;
use bot_core::lifecycle::Shutdown;
use bot_core::models::{BotModule, ExecutionMode, TradeSource, Venue};
use bot_core::oms::{ExecutionRecord, OrderDraft, OrderManager, OrderStatus};
use bot_core::state::Shared;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

pub struct PersistencePump {
    db: Arc<Database>,
    state: Shared,
    shutdown: Arc<Shutdown>,
}

impl PersistencePump {
    /// Spawn the pump task. Returns its handle (joined during shutdown).
    pub fn spawn(
        db: Arc<Database>,
        state: Shared,
        shutdown: Arc<Shutdown>,
    ) -> tokio::task::JoinHandle<()> {
        let pump = PersistencePump {
            db,
            state,
            shutdown,
        };
        tokio::spawn(async move { pump.run().await })
    }

    async fn run(self) {
        let mut rx = self.state.events.subscribe();
        info!("persistence pump started");
        loop {
            tokio::select! {
                _ = self.shutdown.wait() => {
                    // Drain whatever is already buffered, then stop. A hard
                    // cut here would lose the last events before exit.
                    while let Ok(ev) = rx.try_recv() {
                        self.handle(&ev).await;
                    }
                    info!("persistence pump stopped (shutdown)");
                    break;
                }
                res = rx.recv() => {
                    match res {
                        Ok(ev) => self.handle(&ev).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "persistence pump lagged — durable state may miss events; reconciliation will catch up");
                            bot_core::obs::metrics::global()
                                .counter(
                                    "bot_persist_lagged_total",
                                    "Events skipped by the persistence pump due to lag.",
                                    &[],
                                )
                                .inc_by(n);
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }

    fn orders(&self) -> Option<Arc<OrderManager>> {
        self.state.orders().cloned()
    }

    async fn handle(&self, ev: &AppEvent) {
        match ev {
            AppEvent::OrderSent {
                ts,
                module,
                symbol,
                venue,
                mode,
                quote_amount,
                signature,
                signer,
                attempts,
                latency_ms,
            } => {
                self.on_order_sent(
                    *ts,
                    *module,
                    symbol,
                    venue,
                    mode,
                    *quote_amount,
                    signature.as_deref(),
                    signer.as_deref(),
                    *attempts,
                    *latency_ms,
                )
                .await;
            }
            AppEvent::Fill { trade, .. } => self.on_fill(trade).await,
            AppEvent::PositionUpdate { position, .. } => {
                if let Err(e) = PositionRepo::new(self.db.clone()).upsert(position).await {
                    warn!(error = %e, id = %position.id, "position persistence failed");
                }
            }
            AppEvent::PositionClosed { position, .. } => {
                if let Err(e) = PositionRepo::new(self.db.clone()).upsert(position).await {
                    warn!(error = %e, id = %position.id, "closed-position persistence failed");
                }
            }
            AppEvent::RiskRejected {
                module,
                symbol,
                reason,
                ..
            } => {
                let repo = RiskEventRepo::new(self.db.clone());
                if let Err(e) = repo
                    .append(
                        module.as_str(),
                        "rejected",
                        Some(symbol),
                        reason,
                        &serde_json::json!({}),
                    )
                    .await
                {
                    debug!(error = %e, "risk-event persistence failed");
                }
            }
            AppEvent::Error {
                module,
                message,
                fatal,
                ..
            } => {
                let repo = SystemEventRepo::new(self.db.clone());
                let severity = if *fatal { "error" } else { "warn" };
                if let Err(e) = repo
                    .append(
                        "module_error",
                        module.map(|m| m.as_str()),
                        severity,
                        message,
                        &serde_json::json!({}),
                    )
                    .await
                {
                    debug!(error = %e, "system-event persistence failed");
                }
            }
            AppEvent::Lifecycle { message, .. } => {
                let repo = SystemEventRepo::new(self.db.clone());
                if let Err(e) = repo
                    .append("lifecycle", None, "info", message, &serde_json::json!({}))
                    .await
                {
                    debug!(error = %e, "lifecycle persistence failed");
                }
            }
            // Audit events are written by the AuditTrail itself; module
            // status/info/launch/wallet/signal/command events stay in the
            // JSONL journal + metrics (writing them all to SQL would be
            // write amplification without operational value).
            _ => {}
        }
    }

    /// OrderSent → OMS order (Created → Submitted), transaction row and a
    /// reconciliation claim on the signature.
    #[allow(clippy::too_many_arguments)]
    async fn on_order_sent(
        &self,
        ts: chrono::DateTime<chrono::Utc>,
        module: BotModule,
        symbol: &str,
        venue: &str,
        mode: &str,
        quote_amount: f64,
        signature: Option<&str>,
        signer: Option<&str>,
        attempts: Option<u8>,
        latency_ms: Option<u64>,
    ) {
        let Some(mgr) = self.orders() else {
            return;
        };
        // Deterministic intent identity: with a signature it is the anchor;
        // paper/simulated sends key on (module, symbol, ts-millis).
        let idem = match signature {
            Some(sig) => sha256_hex(&format!("ordersent|{module}|{symbol}|{sig}")),
            None => sha256_hex(&format!(
                "ordersent|{module}|{symbol}|{}",
                ts.timestamp_millis()
            )),
        };
        let exec_mode = mode
            .parse::<ExecutionMode>()
            .unwrap_or(ExecutionMode::Paper);
        let draft = OrderDraft {
            idempotency_key: idem,
            module,
            side: "buy".into(), // refined by the Fill event when it arrives
            symbol: symbol.to_string(),
            venue: venue.to_string(),
            mode: exec_mode,
            qty: quote_amount,
            price: None,
            meta: serde_json::json!({
                "quote_amount": quote_amount,
                "source_event": "order_sent",
            }),
        };
        let order = match mgr.create(draft).await {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "OMS create failed for OrderSent event");
                return;
            }
        };
        if order.status == OrderStatus::Created {
            let _ = mgr
                .transition(&order.id, OrderStatus::Submitted, Some("order sent"))
                .await;
        }
        if let Some(sig) = signature {
            // Polymarket submissions carry the derived CLOB order id in
            // `signature`; they reconcile through the venue adapter queue,
            // not the Solana transaction queue.
            let is_poly = venue == "polymarket";
            if is_poly {
                let _ = mgr
                    .attach_external(&order.id, Some(sig.to_string()), None)
                    .await;
            } else {
                let _ = mgr
                    .attach_external(&order.id, None, Some(sig.to_string()))
                    .await;
            }
            let tx_repo = TransactionRepo::new(self.db.clone());
            let chain = if is_poly { "polymarket" } else { "solana" };
            match tx_repo
                .record_submitted(
                    chain,
                    sig,
                    Some(&order.id),
                    signer,
                    Some(venue),
                    i32::from(attempts.unwrap_or(1)),
                )
                .await
            {
                Ok(_) => {
                    let kind = if is_poly {
                        "polymarket_order"
                    } else {
                        "transaction"
                    };
                    if let Err(e) = ReconRepo::new(self.db.clone()).enqueue(kind, sig).await {
                        debug!(error = %e, "recon enqueue failed");
                    }
                }
                Err(e) => warn!(error = %e, %sig, "transaction persistence failed"),
            }
        }
        mgr.record_execution(ExecutionRecord {
            order_id: order.id.clone(),
            ts,
            kind: "send".into(),
            endpoint: Some(venue.to_string()),
            latency_ms,
            ok: true,
            detail: Some(format!("mode={mode} quote={quote_amount}")),
        })
        .await;
    }

    /// Fill → durable trade row + OMS order completion.
    async fn on_fill(&self, trade: &bot_core::models::Trade) {
        if let Err(e) = TradeRepo::new(self.db.clone()).append(trade).await {
            warn!(error = %e, id = %trade.id, "trade persistence failed");
        }
        let Some(mgr) = self.orders() else {
            return;
        };

        // Find the matching order: by signature (solana path) or create the
        // provider order now (polymarket path — its fills are the first
        // durable evidence of the order).
        let mut order = match &trade.signature {
            Some(sig) => OrderRepo::new(self.db.clone())
                .get_by_signature(sig)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        if order.is_none() {
            // In-memory mirror lookup by signature (the DB may have lagged).
            if let Some(sig) = &trade.signature {
                for o in mgr.list(500).await {
                    if o.signature.as_deref() == Some(sig.as_str()) {
                        order = Some(o);
                        break;
                    }
                }
            }
        }

        let side = if trade.is_buy() { "buy" } else { "sell" };
        let order = match order {
            Some(o) => o,
            None => {
                // Polymarket / paper fills without a preceding OrderSent.
                let external = extract_poly_order_id(trade.note.as_deref());
                let idem = sha256_hex(&format!(
                    "fill|{}|{}|{}",
                    trade.source, trade.symbol, trade.id
                ));
                let draft = OrderDraft {
                    idempotency_key: idem,
                    module: match trade.source {
                        TradeSource::Sniper => BotModule::Sniper,
                        TradeSource::Copy => BotModule::Copy,
                        TradeSource::Polymarket => BotModule::Polymarket,
                        _ => BotModule::Sniper,
                    },
                    side: side.to_string(),
                    symbol: trade.symbol.clone(),
                    venue: trade.venue.as_str().to_string(),
                    mode: trade.mode,
                    qty: trade.amount_out,
                    price: Some(trade.price),
                    meta: serde_json::json!({
                        "trade_id": trade.id,
                        "source_event": "fill",
                        "amount_in": trade.amount_in,
                    }),
                };
                match mgr.create(draft).await {
                    Ok(o) => {
                        if let Some(ext) = external {
                            let _ = mgr
                                .attach_external(&o.id, Some(ext.clone()), trade.signature.clone())
                                .await;
                            // Verify the provider agrees this order matched.
                            if trade.source == TradeSource::Polymarket {
                                if let Err(e) = ReconRepo::new(self.db.clone())
                                    .enqueue("polymarket_order", &ext)
                                    .await
                                {
                                    debug!(error = %e, "poly recon enqueue failed");
                                }
                            }
                        } else if let Some(sig) = &trade.signature {
                            let _ = mgr.attach_external(&o.id, None, Some(sig.clone())).await;
                        }
                        o
                    }
                    Err(e) => {
                        warn!(error = %e, "OMS create failed for Fill event");
                        return;
                    }
                }
            }
        };

        // Drive the lifecycle to its terminal fill state.
        if !order.status.is_terminal() {
            let target = OrderStatus::Filled;
            if let Err(e) = mgr
                .transition(&order.id, target, Some("fill observed"))
                .await
            {
                // Illegal from the current status (e.g. still Created): walk
                // the canonical path first.
                let _ = mgr
                    .transition(&order.id, OrderStatus::Submitted, Some("fill recovery"))
                    .await;
                if let Err(e2) = mgr
                    .transition(&order.id, target, Some("fill observed"))
                    .await
                {
                    debug!(error = %e, error2 = %e2, order = %order.id, "fill transition failed");
                }
            }
        }
        // Claim the fill's signature for reconciliation if it is not already
        // claimed (exit paths publish Fill without a preceding OrderSent).
        // Solana signatures only — Polymarket fills are claimed by CLOB order
        // id via the note-based path above. ON CONFLICT DO NOTHING keeps this
        // idempotent with the OrderSent claim.
        if let Some(sig) = trade
            .signature
            .as_deref()
            .filter(|s| !s.is_empty() && trade.venue != Venue::PolymarketClob)
        {
            let tx_repo = TransactionRepo::new(self.db.clone());
            match tx_repo
                .record_submitted(
                    "solana",
                    sig,
                    Some(&order.id),
                    None,
                    Some(trade.venue.as_str()),
                    1,
                )
                .await
            {
                Ok(_) => {
                    if let Err(e) = ReconRepo::new(self.db.clone())
                        .enqueue("transaction", sig)
                        .await
                    {
                        debug!(error = %e, "fill recon enqueue failed");
                    }
                }
                Err(e) => warn!(error = %e, "fill transaction persistence failed"),
            }
        }
        mgr.record_execution(ExecutionRecord {
            order_id: order.id.clone(),
            ts: trade.ts,
            kind: "confirm".into(),
            endpoint: Some(trade.venue.as_str().to_string()),
            latency_ms: trade.latency_ms,
            ok: true,
            detail: Some(format!(
                "fill {side} qty={} price={} trade={}",
                trade.amount_out, trade.price, trade.id
            )),
        })
        .await;
    }
}

/// Extract the CLOB order id from the note format used by module 3
/// (`"<reason> order=<id>"`).
pub fn extract_poly_order_id(note: Option<&str>) -> Option<String> {
    let note = note?;
    let idx = note.find("order=")?;
    let rest = &note[idx + "order=".len()..];
    let id: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// Restart recovery: reload open positions + unfinished orders into memory
/// and re-enqueue unverified transactions. Runs BEFORE modules start.
pub async fn restore(db: &Arc<Database>, state: &Shared) {
    // 1) Positions.
    match PositionRepo::new(db.clone()).list_open().await {
        Ok(positions) => {
            let n = state.restore_positions(positions).await;
            if n > 0 {
                info!(restored = n, "open positions restored from the database");
            }
        }
        Err(e) => warn!(error = %e, "position restore failed — starting with an empty book"),
    }

    // 2) Orders (non-terminal → Unknown, reconcilers resolve them).
    if let Some(mgr) = state.orders() {
        match mgr.recover_from_db().await {
            Ok(n) if n > 0 => info!(restored = n, "unfinished orders recovered as Unknown"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "order recovery failed"),
        }
    }

    // 3) Transactions whose confirmation was never observed → recon queue.
    let swept =
        bot_core::recovery::sweep_unresolved_transactions(db, Duration::from_secs(90)).await;
    if swept > 0 {
        info!(swept, "unresolved transactions enqueued for reconciliation");
    }
}

// ---------------------------------------------------------------------------
// JSONL journal pump
// ---------------------------------------------------------------------------

/// Rotate a journal when it grows past this size (per file).
const ROTATE_AT_BYTES: u64 = 64 * 1024 * 1024;

/// The JSONL journal consumer: every event is appended to `events.jsonl`,
/// fills to `trades.jsonl` and position changes to `positions.jsonl`.
/// Crash-safe by design (append + flush per line); oversized journals are
/// rotated (archived with a timestamp suffix) instead of growing forever.
pub struct JournalPump {
    store: bot_core::storage::Store,
    state: Shared,
    shutdown: Arc<Shutdown>,
}

impl JournalPump {
    pub fn spawn(
        store: bot_core::storage::Store,
        state: Shared,
        shutdown: Arc<Shutdown>,
    ) -> tokio::task::JoinHandle<()> {
        let pump = JournalPump {
            store,
            state,
            shutdown,
        };
        tokio::spawn(async move { pump.run().await })
    }

    async fn run(self) {
        use bot_core::storage::JournalKind;
        let mut rx = self.state.events.subscribe();
        let mut rotate_ticker = tokio::time::interval(Duration::from_secs(60));
        rotate_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        info!(dir = %self.store.dir().display(), "jsonl journal pump started");
        loop {
            tokio::select! {
                _ = self.shutdown.wait() => {
                    while let Ok(ev) = rx.try_recv() {
                        self.write(&ev).await;
                    }
                    info!("journal pump stopped (shutdown)");
                    break;
                }
                _ = rotate_ticker.tick() => {
                    self.rotate_if_needed(JournalKind::Events).await;
                    self.rotate_if_needed(JournalKind::Trades).await;
                    self.rotate_if_needed(JournalKind::Positions).await;
                }
                res = rx.recv() => {
                    match res {
                        Ok(ev) => self.write(&ev).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "journal pump lagged — journal has a gap");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }

    async fn write(&self, ev: &AppEvent) {
        // Append the typed journals first (money rows), then the event log.
        match ev {
            AppEvent::Fill { trade, .. } => {
                if let Err(e) = self.store.append_trade(trade).await {
                    warn!(error = %e, "trade journal append failed");
                }
            }
            AppEvent::PositionUpdate { position, .. }
            | AppEvent::PositionClosed { position, .. } => {
                if let Err(e) = self.store.append_position(position).await {
                    warn!(error = %e, "position journal append failed");
                }
            }
            _ => {}
        }
        if let Err(e) = self.store.append_event(ev).await {
            debug!(error = %e, "event journal append failed");
        }
    }

    async fn rotate_if_needed(&self, kind: bot_core::storage::JournalKind) {
        let path = match kind {
            bot_core::storage::JournalKind::Trades => self.store.trades_path().to_path_buf(),
            bot_core::storage::JournalKind::Positions => self.store.positions_path().to_path_buf(),
            bot_core::storage::JournalKind::Events => self.store.events_path().to_path_buf(),
        };
        if self.store.size_of(&path).await > ROTATE_AT_BYTES {
            match self.store.rotate(kind).await {
                Ok(Some(archive)) => info!(?kind, archive = %archive.display(), "journal rotated"),
                Ok(None) => {}
                Err(e) => warn!(error = %e, ?kind, "journal rotation failed"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poly_order_id_extraction_matches_module_format() {
        assert_eq!(
            extract_poly_order_id(Some("momentum burst order=0xabc123")),
            Some("0xabc123".into())
        );
        assert_eq!(extract_poly_order_id(Some("order=abc")), Some("abc".into()));
        assert_eq!(extract_poly_order_id(Some("no id here")), None);
        assert_eq!(extract_poly_order_id(Some("order=")), None);
        assert_eq!(extract_poly_order_id(None), None);
        assert_eq!(
            extract_poly_order_id(Some("x order=id1 trailing")),
            Some("id1".into()),
            "stops at whitespace"
        );
    }
}
