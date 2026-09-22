//! Venue-order lifecycle tracking and fill accounting.
//!
//! Every fact about a tracked order enters through ONE door —
//! [`PolyBot::apply_observation`] — whether it came from the POST answer, a
//! status poll ([`PolyBot::poll_orders_once`]), the authenticated user channel
//! ([`PolyBot::apply_user_event`]), a cancel answer or reconciliation. The
//! pure state machine is [`TrackedOrder::apply_venue`] (`orders.rs`); this
//! module books what it reports: the durable fill row (replay guard), the
//! trade, the position, the OMS transition, the journal snapshot, metrics
//! (`poly_order_transitions_total`, `poly_orders_total`, `poly_fills_total`,
//! `poly_cancel_total`) and the `poly.order.<state>` audit record.
//!
//! Cancels are applied locally only when the venue confirms them; a
//! definite local failure or a vanished order goes through
//! [`PolyBot::finish_locally`], never through an assumed fill.

use chrono::Utc;
use tracing::{debug, info, warn};

use bot_core::config::PolymarketConfig;
use bot_core::events::AppEvent;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::oms::{ExecutionRecord, OrderStatus};

use crate::clob::CancelOutcome;
use crate::error::{PolyError, PolyResult};
use crate::metrics;
use crate::orders::{
    FillSource, LifecycleEffect, LocalOrderState, TrackedOrder, VenueObservation, VenueOrderState,
};
use crate::store::PolyFillRecord;
use crate::ws::UserEvent;
use crate::PolyBot;

/// Terminal orders stay in the tracker this long (operator visibility) and
/// are then pruned.
const TERMINAL_RETENTION_SECS: i64 = 3_600;

impl PolyBot {
    /// FAK follow-up after a status-only `matched` POST answer: one
    /// `GET /data/order` for the matched quantity, applied through the same
    /// lifecycle machine as a poll. A venue that cannot answer (no
    /// credentials, `404`, transport error) leaves the order open with its
    /// `matched` acknowledgement for polling / the user channel /
    /// reconciliation — the quantity is never assumed.
    pub(crate) async fn confirm_fill_and_kill_quantity(&self, tracked: &mut TrackedOrder) {
        let client = match self.authed_client().await {
            Ok(c) => c,
            Err(e) => {
                debug!(order = %tracked.venue_order_id, error = %e, "fak quantity lookup skipped");
                return;
            }
        };
        match client.order(&tracked.venue_order_id).await {
            Ok(Some(venue)) => {
                let obs = VenueObservation {
                    state: venue.state(),
                    raw_status: venue.status.clone(),
                    size_matched: venue.size_matched_opt(),
                    fill_delta: None,
                    trade_id: None,
                    price: None,
                    source: FillSource::Poll,
                    at: Utc::now(),
                    associate_trades: venue.associate_trades.clone(),
                };
                if let Err(e) = self.apply_observation(tracked, &obs).await {
                    warn!(order = %tracked.venue_order_id, error = %e, "fak quantity observation rejected");
                }
            }
            Ok(None) => {
                debug!(
                    order = %tracked.venue_order_id,
                    "fak matched on submit but not yet readable; quantity left to poll / user channel"
                );
            }
            Err(e) => {
                debug!(order = %tracked.venue_order_id, error = %e, "fak quantity lookup failed");
            }
        }
    }

    /// Apply a venue observation to `tracked`: book any new fill (trade,
    /// position, events, journal), transition the OMS order and journal the
    /// order snapshot. The tracker map is updated with the result.
    ///
    /// The durable fill journal is the replay authority across restarts:
    /// when it already holds the fill of a **per-trade** observation (a
    /// user-channel `trade` re-emitted after a restart, whose id the
    /// rebuilt tracker no longer remembers), the observation is discarded —
    /// the local cumulative and state stay where they were and only the
    /// trade id is remembered. A **cumulative** observation whose fill row
    /// already exists still advances the local snapshot (the venue's
    /// cumulative is the truth; the ledger was written before the crash)
    /// without booking the trade or the position a second time.
    pub(crate) async fn apply_observation(
        &self,
        tracked: &mut TrackedOrder,
        obs: &VenueObservation,
    ) -> PolyResult<LifecycleEffect> {
        let mut next = tracked.clone();
        let effect = next.apply_venue(obs)?;
        if effect.fill_delta > 0.0 {
            let booked = self
                .book_fill(
                    &mut next,
                    effect.fill_delta,
                    effect.fill_price,
                    obs.source,
                    effect.trade_id.as_deref(),
                )
                .await;
            if !booked && obs.fill_delta.is_some() {
                if let Some(id) = effect.trade_id.as_deref() {
                    tracked.remember_trade(id);
                }
                tracked.updated_at = obs.at;
                self.insert_tracked(tracked.clone()).await;
                self.journal_order(tracked).await;
                return Ok(LifecycleEffect {
                    fill_delta: 0.0,
                    fill_price: effect.fill_price,
                    from: tracked.state,
                    to: tracked.state,
                    transitioned: false,
                    trade_id: None,
                });
            }
        }
        *tracked = next;
        if effect.transitioned {
            metrics::count_order_transition(effect.from.as_str(), effect.to.as_str());
            let reason = format!("venue {} via {}", obs.raw_status, obs.source.as_str());
            self.oms_transition(&tracked.order_id, effect.to.oms_status(), &reason)
                .await;
            if effect.to.is_terminal() {
                self.audit(
                    &format!("poly.order.{}", effect.to.as_str()),
                    &tracked.venue_order_id,
                    &format!(
                        "{} token={} matched={:.2}/{:.2} source={}",
                        effect.to.as_str(),
                        tracked.token_id,
                        tracked.size_matched,
                        tracked.size_tokens,
                        obs.source.as_str()
                    ),
                );
                metrics::count_order_terminal(effect.to.as_str());
            }
        }
        self.insert_tracked(tracked.clone()).await;
        self.journal_order(tracked).await;
        Ok(effect)
    }

    /// Book one fill: journal (replay guard), trade, position, events.
    /// Returns `false` when the durable journal already held this fill (the
    /// trade and position were NOT booked again).
    async fn book_fill(
        &self,
        tracked: &mut TrackedOrder,
        delta: f64,
        price: f64,
        source: FillSource,
        trade_id: Option<&str>,
    ) -> bool {
        let fill_id = tracked.fill_id(trade_id, source);
        let quote = delta * price;
        let now = Utc::now();
        let mode = tracked.mode;

        // Position first (so the fill row can carry the id), then the
        // durable fill claim, then the trade/events.
        let position_id = match tracked.position_id.clone() {
            Some(id) => id,
            None => match self
                .state
                .find_open(BotModule::Polymarket, &tracked.token_id)
                .await
            {
                Some(p) => p.id,
                None => self.state.next_id("p"),
            },
        };
        let rec = PolyFillRecord {
            fill_id: fill_id.clone(),
            venue_order_id: tracked.venue_order_id.clone(),
            order_id: tracked.order_id.clone(),
            token_id: tracked.token_id.clone(),
            side: if tracked.is_buy { "buy" } else { "sell" }.into(),
            price,
            size_tokens: delta,
            quote_usd: quote,
            source: source.as_str().into(),
            position_id: Some(position_id.clone()),
            ts: now,
        };
        match self.store.record_fill(rec).await {
            Some(true) => {}
            Some(false) => {
                debug!(fill = %fill_id, "fill already journaled — not booked twice");
                metrics::count_duplicate_prevented("poly_fill_journal");
                return false;
            }
            None => {
                metrics::count_journal_error("record_fill");
            }
        }
        metrics::count_fill(source.as_str());

        let trade = Trade {
            id: self.state.next_id("t"),
            ts: now,
            source: TradeSource::Polymarket,
            venue: Venue::PolymarketClob,
            mode,
            side: if tracked.is_buy {
                PositionSide::Long
            } else {
                PositionSide::Short
            },
            symbol: tracked.token_id.clone(),
            symbol_display: format!("{} {}", tracked.question, tracked.outcome),
            amount_in: if tracked.is_buy { quote } else { delta },
            amount_out: if tracked.is_buy { delta } else { quote },
            quote_symbol: "USDC".into(),
            price,
            fee: 0.0,
            slippage_bps: 0,
            signature: tracked.signature.clone(),
            position_id: Some(position_id.clone()),
            note: Some(format!(
                "{} order={} oms={} fill={}",
                tracked.outcome, tracked.venue_order_id, tracked.order_id, fill_id
            )),
            latency_ms: None,
        };
        // TASK 5 — the typed accounting event for this fill. Built from the
        // trade record so both carry the same figures; the venue fill id is
        // the reference (a replayed user-channel trade or a repeated poll
        // that slipped past the fill journal is a ledger duplicate, never
        // a second booking). Submitted below, after the module position.
        let ledger_event = bot_core::accounting::fill_event_for_trade(
            &trade,
            self.ledger_wallet(),
            self.strategy_label().await,
            Some(fill_id.clone()),
            Some(tracked.order_id.clone()),
        );
        // `record_trade` publishes the `Fill` event (persistence + UI).
        self.state.record_trade(trade).await;

        let existing = self.state.position(&position_id).await;
        let position = match existing {
            Some(_) => {
                self.state
                    .with_position(&position_id, |p| {
                        if tracked.is_buy {
                            p.apply_buy(delta, price, quote);
                        } else {
                            p.apply_sell(delta, price, quote);
                        }
                    })
                    .await
            }
            None => {
                let mut position = Position::new(
                    position_id.clone(),
                    TradeSource::Polymarket,
                    Venue::PolymarketClob,
                    mode,
                    tracked.token_id.clone(),
                    tracked.outcome.clone(),
                    "USDC".into(),
                );
                position.apply_buy(delta, price, quote);
                position.market_id = Some(tracked.condition_id.clone());
                position.outcome = Some(tracked.outcome.clone());
                position.entry_signature = tracked.signature.clone();
                // Exit at redemption (1.0) or a stop below entry.
                position.take_profit = Some(0.99);
                position.stop_loss = Some((price * 0.5).max(0.01));
                self.state.upsert_position(position.clone()).await;
                Some(position)
            }
        };
        if let Some(position) = position {
            if !tracked.is_buy && position.qty <= 1e-9 {
                self.state
                    .close_position(&position.id, PositionStatus::Closed, "sold on clob")
                    .await;
            } else {
                self.state.events.publish(AppEvent::PositionUpdate {
                    ts: Utc::now(),
                    position: Box::new(position),
                });
            }
        }
        tracked.position_id = Some(position_id);
        // The global ledger is the only mutator of global accounting state;
        // the module hands over the typed event and keeps its own record.
        self.state.ledger().submit(ledger_event).await;
        self.orders
            .record_execution(ExecutionRecord {
                order_id: tracked.order_id.clone(),
                ts: now,
                kind: "fill".into(),
                endpoint: Some(source.as_str().into()),
                latency_ms: None,
                ok: true,
                detail: Some(format!("{delta:.2} @ {price:.4} ({fill_id})")),
            })
            .await;
        info!(
            outcome = %tracked.outcome,
            token = %tracked.token_id,
            price,
            size = delta,
            matched = tracked.size_matched,
            of = tracked.size_tokens,
            source = source.as_str(),
            mode = %mode.as_str(),
            "polymarket fill booked"
        );
        true
    }

    /// Poll the venue for every tracked non-terminal LIVE order, apply the
    /// answers, then enforce local TTL / GTD expiry / reprice rules by
    /// cancelling. Returns the number of orders polled.
    pub async fn poll_orders_once(&self, poly: &PolymarketConfig) -> PolyResult<usize> {
        self.prune_terminal().await;
        let open: Vec<TrackedOrder> = self
            .tracked
            .read()
            .await
            .values()
            .filter(|t| !t.state.is_terminal() && t.mode == ExecutionMode::Live)
            .cloned()
            .collect();
        if open.is_empty() {
            return Ok(0);
        }
        let client = match self.authed_client().await {
            Ok(c) => c,
            Err(e) => {
                debug!(error = %e, "cannot poll polymarket orders without credentials");
                return Ok(0);
            }
        };
        let now = Utc::now();
        let mut polled = 0usize;
        for mut tracked in open {
            polled += 1;
            match client.order(&tracked.venue_order_id).await {
                Ok(Some(venue)) => {
                    let obs = VenueObservation {
                        state: venue.state(),
                        raw_status: venue.status.clone(),
                        size_matched: venue.size_matched_opt(),
                        fill_delta: None,
                        trade_id: None,
                        price: None,
                        source: FillSource::Poll,
                        at: now,
                        associate_trades: venue.associate_trades.clone(),
                    };
                    if let Err(e) = self.apply_observation(&mut tracked, &obs).await {
                        warn!(order = %tracked.venue_order_id, error = %e, "poll observation rejected");
                        continue;
                    }
                }
                Ok(None) => {
                    let never_acknowledged = matches!(
                        tracked.state,
                        LocalOrderState::Submitted | LocalOrderState::Unknown
                    ) && !tracked.venue_acknowledged();
                    if never_acknowledged {
                        // Never accepted and not on the venue: definite.
                        if tracked.age_secs(now) >= poly.order_poll_interval_secs as i64 {
                            self.finish_locally(
                                &mut tracked,
                                LocalOrderState::Failed,
                                "not on venue",
                            )
                            .await;
                        }
                    } else if tracked.state != LocalOrderState::Unknown {
                        // Was accepted before (resting, partially filled, or
                        // a `matched` FAK whose quantity is still unread);
                        // the venue forgot it — do not invent a fill or a
                        // cancel: mark unknown for reconciliation.
                        self.finish_locally(
                            &mut tracked,
                            LocalOrderState::Unknown,
                            "vanished from venue",
                        )
                        .await;
                    }
                    continue;
                }
                Err(e) => {
                    debug!(order = %tracked.venue_order_id, error = %e, "order poll failed");
                    continue;
                }
            }
            if tracked.state.is_terminal() {
                continue;
            }
            // Local rules: TTL, GTD expiry passed, reprice.
            let mut cancel_reason: Option<&str> = None;
            if tracked.ttl_elapsed(now, poly.order_ttl_secs) {
                cancel_reason = Some("ttl");
            } else if tracked.expiry_passed(now) {
                cancel_reason = Some("expired");
            } else if poly.reprice_threshold > 0.0 {
                let best_ask = self
                    .quotes
                    .read()
                    .await
                    .get(&tracked.token_id)
                    .map(|q| q.best_ask)
                    .unwrap_or(0.0);
                if tracked.needs_reprice(best_ask, poly.reprice_threshold) {
                    cancel_reason = Some("reprice");
                }
            }
            if let Some(reason) = cancel_reason {
                match self
                    .cancel_tracked_order(&tracked.venue_order_id, reason)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        debug!(order = %tracked.venue_order_id, reason, "cancel not confirmed")
                    }
                    Err(e) => {
                        warn!(order = %tracked.venue_order_id, reason, error = %e, "cancel failed")
                    }
                }
            }
        }
        Ok(polled)
    }

    /// Apply one authenticated user-channel event to the tracked orders.
    /// Unknown order ids are ignored (they may belong to another process
    /// sharing the API key) — reconciliation reports them as orphans.
    pub async fn apply_user_event(&self, ev: &UserEvent) -> PolyResult<()> {
        match ev {
            UserEvent::Order {
                order_id,
                state,
                raw_status,
                size_matched,
                ts,
                associate_trades,
                ..
            } => {
                let Some(mut tracked) = self.get_tracked(order_id).await else {
                    debug!(order = %order_id, "user ws order event for untracked order");
                    return Ok(());
                };
                let obs = VenueObservation {
                    state: *state,
                    raw_status: raw_status.clone(),
                    size_matched: *size_matched,
                    fill_delta: None,
                    trade_id: None,
                    price: None,
                    source: FillSource::UserWs,
                    at: ts.unwrap_or_else(Utc::now),
                    associate_trades: associate_trades.clone(),
                };
                self.apply_observation(&mut tracked, &obs).await?;
                Ok(())
            }
            UserEvent::Trade {
                trade_id,
                taker_order_id,
                maker_fills,
                size,
                price,
                ts,
                ..
            } => {
                if ev.is_failed_trade() {
                    debug!(trade = %trade_id, "user ws trade FAILED — not booked");
                    return Ok(());
                }
                let at = ts.unwrap_or_else(Utc::now);
                let mut touched: Vec<(String, f64, f64)> = Vec::new();
                if let Some(id) = taker_order_id {
                    touched.push((id.clone(), *size, *price));
                }
                for m in maker_fills {
                    touched.push((m.order_id.clone(), m.matched, m.price));
                }
                for (order_id, delta, px) in touched {
                    let Some(mut tracked) = self.get_tracked(&order_id).await else {
                        continue;
                    };
                    let obs = VenueObservation {
                        state: VenueOrderState::Live,
                        raw_status: "trade".into(),
                        size_matched: None,
                        fill_delta: Some(delta),
                        trade_id: Some(trade_id.clone()),
                        price: Some(px),
                        source: FillSource::UserWs,
                        at,
                        associate_trades: Vec::new(),
                    };
                    self.apply_observation(&mut tracked, &obs).await?;
                }
                Ok(())
            }
        }
    }

    /// Cancel one tracked venue order (`DELETE /order`). Returns `true` when
    /// the venue confirmed the cancel (the order is then `Cancelled`
    /// locally; a partial fill stays booked). Paper orders cannot be
    /// cancelled (they fill instantly).
    pub async fn cancel_tracked_order(
        &self,
        venue_order_id: &str,
        reason: &str,
    ) -> PolyResult<bool> {
        let Some(mut tracked) = self.get_tracked(venue_order_id).await else {
            return Err(PolyError::lifecycle(format!(
                "{venue_order_id} is not tracked"
            )));
        };
        if tracked.state.is_terminal() {
            return Ok(false);
        }
        if tracked.mode != ExecutionMode::Live {
            return Ok(false);
        }
        let client = self.authed_client().await?;
        let v = client.cancel_order(&tracked.venue_order_id).await?;
        let outcome = CancelOutcome::from_value(&v);
        metrics::count_cancel(reason);
        if outcome.confirmed(&tracked.venue_order_id) {
            self.finish_locally(&mut tracked, LocalOrderState::Cancelled, reason)
                .await;
            return Ok(true);
        }
        if let Some((_, why)) = outcome
            .not_canceled
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(&tracked.venue_order_id))
        {
            // Refused: the order stays open on the venue and locally; the
            // next poll retries.
            debug!(order = %tracked.venue_order_id, why = %why, "cancel refused by venue");
            return Ok(false);
        }
        // The answer named neither list (unrecognised shape / empty body):
        // that is NOT a confirmation. Ask the venue for the order itself and
        // apply whatever it says through the lifecycle machine; an order the
        // venue still reports as open stays open and is retried next poll.
        match client.order(&tracked.venue_order_id).await {
            Ok(Some(venue)) => {
                let venue_state = venue.state();
                let obs = VenueObservation {
                    state: venue_state,
                    raw_status: venue.status.clone(),
                    size_matched: venue.size_matched_opt(),
                    fill_delta: None,
                    trade_id: None,
                    price: None,
                    source: FillSource::Poll,
                    at: Utc::now(),
                    associate_trades: venue.associate_trades.clone(),
                };
                if let Err(e) = self.apply_observation(&mut tracked, &obs).await {
                    warn!(order = %tracked.venue_order_id, error = %e, "post-cancel observation rejected");
                    return Ok(false);
                }
                let confirmed = matches!(
                    venue_state,
                    VenueOrderState::Cancelled | VenueOrderState::Expired
                );
                if !confirmed {
                    debug!(
                        order = %tracked.venue_order_id,
                        venue_status = %venue.status,
                        "cancel unconfirmed: venue answer unrecognised and order still reported"
                    );
                }
                Ok(confirmed)
            }
            Ok(None) => {
                // Gone from the venue right after our cancel — still not a
                // confirmation: the poll's "vanished" path and
                // reconciliation own the outcome, never a local guess.
                debug!(order = %tracked.venue_order_id, "cancel unconfirmed: venue no longer reports the order");
                Ok(false)
            }
            Err(e) => {
                debug!(order = %tracked.venue_order_id, error = %e, "cancel unconfirmed: status lookup failed");
                Ok(false)
            }
        }
    }

    /// Cancel every tracked non-terminal LIVE order. Returns how many the
    /// venue confirmed.
    pub async fn cancel_all_tracked(&self, reason: &str) -> usize {
        let open: Vec<String> = self
            .tracked
            .read()
            .await
            .values()
            .filter(|t| !t.state.is_terminal() && t.mode == ExecutionMode::Live)
            .map(|t| t.venue_order_id.clone())
            .collect();
        let mut n = 0usize;
        for id in open {
            match self.cancel_tracked_order(&id, reason).await {
                Ok(true) => n += 1,
                Ok(false) => {}
                Err(e) => warn!(order = %id, error = %e, "cancel failed"),
            }
        }
        n
    }

    /// Force a local terminal / unknown state (our cancel confirmed, a
    /// definite failure, a vanished order) and propagate to OMS + journal.
    pub(crate) async fn finish_locally(
        &self,
        tracked: &mut TrackedOrder,
        to: LocalOrderState,
        reason: &str,
    ) {
        let from = tracked.state;
        match tracked.force_state(to, reason, Utc::now()) {
            Ok(true) => {
                metrics::count_order_transition(from.as_str(), to.as_str());
                self.oms_transition(&tracked.order_id, to.oms_status(), reason)
                    .await;
                if to.is_terminal() {
                    metrics::count_order_terminal(to.as_str());
                    self.audit(
                        &format!("poly.order.{}", to.as_str()),
                        &tracked.venue_order_id,
                        &format!(
                            "{} reason={} token={} matched={:.2}/{:.2}",
                            to.as_str(),
                            reason,
                            tracked.token_id,
                            tracked.size_matched,
                            tracked.size_tokens
                        ),
                    );
                }
            }
            Ok(false) => {}
            Err(e) => {
                warn!(order = %tracked.venue_order_id, error = %e, "local transition refused");
                return;
            }
        }
        self.insert_tracked(tracked.clone()).await;
        self.journal_order(tracked).await;
    }

    pub(crate) async fn get_tracked(&self, venue_order_id: &str) -> Option<TrackedOrder> {
        let tracked = self.tracked.read().await;
        tracked
            .get(venue_order_id)
            .or_else(|| {
                tracked
                    .values()
                    .find(|t| t.venue_order_id.eq_ignore_ascii_case(venue_order_id))
            })
            .cloned()
    }

    pub(crate) async fn insert_tracked(&self, tracked: TrackedOrder) {
        self.tracked
            .write()
            .await
            .insert(tracked.venue_order_id.clone(), tracked);
    }

    pub(crate) async fn open_order_count(&self) -> usize {
        self.tracked
            .read()
            .await
            .values()
            .filter(|t| !t.state.is_terminal())
            .count()
    }

    async fn prune_terminal(&self) {
        let now = Utc::now();
        self.tracked.write().await.retain(|_, t| {
            !(t.state.is_terminal()
                && now.signed_duration_since(t.updated_at).num_seconds() > TERMINAL_RETENTION_SECS)
        });
    }

    pub(crate) async fn oms_transition(&self, order_id: &str, to: OrderStatus, reason: &str) {
        if let Err(e) = self.orders.transition(order_id, to, Some(reason)).await {
            debug!(order = %order_id, to = %to.as_str(), error = %e, "oms transition skipped");
        }
    }

    pub(crate) async fn fail_oms(&self, order_id: &str, reason: &str) {
        self.oms_transition(order_id, OrderStatus::Failed, reason)
            .await;
    }
}
