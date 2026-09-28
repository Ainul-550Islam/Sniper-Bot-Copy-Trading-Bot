//! Reconciliation truth sources (BUILD PLAN §11 / Prompt 2 §B,§C,§E,§J,§O).
//!
//! Implementations of [`bot_core::recovery::TruthSource`] for the venues
//! this suite trades on. Each source answers one question — "what does the
//! OUTSIDE WORLD say about this subject?" — decides through the pure
//! comparison engine in [`bot_core::reconciliation`] (one canonical model,
//! typed outcomes, no competing position logic), and applies the resulting
//! state corrections itself (OMS transitions, transaction rows, risk events),
//! so the generic worker in core never needs venue knowledge.
//!
//! Registered kinds:
//! * `transaction`       — a Solana signature: confirmed / failed / pending.
//! * `polymarket_order`  — a CLOB order id: matched / live / cancelled.
//! * `position`          — a live-mode Solana position vs. its aggregated
//!   on-chain token balance (drift detection + conservative corrections;
//!   paper positions have no chain truth).
//! * `intent`            — a write-ahead pre-broadcast intent that was never
//!   linked to a signature (crash point C): ambiguous forever until an
//!   operator or a later link resolves it — never resubmitted.
//!
//! Correction policy (§J/§Y): corrections are only ever derived from
//! AUTHORITATIVE data — confirmed fills for PnL, chain-validated balances
//! for quantities — and every correction is auditable (risk event + event
//! bus + logs). Divergences that authoritative data cannot explain are
//! parked for operators ([`ReconVerdict::GiveUp`]), never silently patched.
//! An unreadable external source is NEVER a conclusion (§O): it maps to
//! [`ReconVerdict::Retry`] plus an `bot_external_state_read_errors_total`
//! sample.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bot_core::db::repo::{
    IntentRecord, IntentRepo, OrderRepo, PositionRepo, ReconRepo, RiskEventRepo, TradeRepo,
    TransactionRepo,
};
use bot_core::db::Database;
use bot_core::models::{ExecutionMode, PositionStatus};
use bot_core::obs::metrics;
use bot_core::oms::OrderStatus;
use bot_core::reconciliation::{
    classify_execution, compare_position, reconstruct_pnl, BalanceObservation,
    ExternalExecutionState, ExternalState, FillRecord, LocalExecutionState, PositionSnapshot,
    QuantityTolerance, ReconOutcome,
};
use bot_core::recovery::{IntentSink, ReconVerdict, TruthSource};
use bot_core::state::Shared;
use solana_kit::rpc::{ConfirmOutcome, Rpc};
use solana_kit::tokens::token_balances_for_owner;
use solana_sdk::signature::Signature;
use tracing::{debug, info, warn};

/// `bot_reconciliation_outcomes_total{kind,outcome}` — low-cardinality typed
/// outcome meter (§T). Labels come from the fixed [`ReconOutcome`] set; no
/// ids, signatures or addresses ever reach the registry.
fn meter_outcome(kind: &str, outcome: &ReconOutcome) {
    metrics::global()
        .counter(
            "bot_reconciliation_outcomes_total",
            "Reconciliation outcomes by kind and typed result.",
            &[("kind", kind), ("outcome", outcome.as_str())],
        )
        .inc();
}

/// `bot_external_state_read_errors_total{source}` — external truth could not
/// be read (§O/§T). Distinct from "read succeeded and says zero/absent".
fn meter_read_error(source: &str) {
    metrics::global()
        .counter(
            "bot_external_state_read_errors_total",
            "Failed external-state reads by source (never a zero-balance conclusion).",
            &[("source", source)],
        )
        .inc();
}

/// Resolve Solana transaction signatures against the cluster (§E/§F).
pub struct SolanaTxTruth {
    rpc: Rpc,
    state: Shared,
    db: Arc<Database>,
}

impl SolanaTxTruth {
    pub fn new(rpc: Rpc, state: Shared, db: Arc<Database>) -> Self {
        SolanaTxTruth { rpc, state, db }
    }

    /// One confirmation probe (short budget — the worker retries with
    /// backoff, so there is no need to block long here). A transport failure
    /// is returned as `Err` — it must NOT collapse into "still pending",
    /// because "could not read" is not an observation (§O).
    async fn probe(&self, sig: &Signature) -> Result<ConfirmOutcome, String> {
        self.rpc
            .confirm(sig, Duration::from_secs(10), Duration::from_secs(2))
            .await
            .map_err(|e| e.to_string())
    }

    /// The LOCAL side of the matrix: what our durable records claim about
    /// this signature.
    async fn local_state(&self, signature: &str) -> LocalExecutionState {
        match TransactionRepo::new(self.db.clone())
            .get_status(signature)
            .await
        {
            Ok(Some(status)) => match status.as_str() {
                "confirmed" | "finalized" => LocalExecutionState::RecordedConfirmed,
                "failed" | "not_found" => LocalExecutionState::RecordedFailed,
                _ => LocalExecutionState::SubmittedUnconfirmed,
            },
            // No row (write lost before crash) but a claim exists → we know
            // we sent something and have no record of the outcome.
            Ok(None) => LocalExecutionState::NoLocalRecord,
            // DB unreadable: stay conservative — treat as unconfirmed; the
            // worker's retry/backoff covers transient DB faults.
            Err(_) => LocalExecutionState::SubmittedUnconfirmed,
        }
    }

    async fn finish_order(&self, signature: &str, filled: bool, detail: &str) {
        // Close the execution lifecycle first: the intent that produced this
        // signature (if this process still tracks it — hydrated from
        // `execution_lifecycle` on restart) moves to `reconciled`, which also
        // releases its duplicate-protection slot. Unknown signatures (legacy
        // broadcasts, other processes) are simply not ours to close.
        if let Some(rec) = bot_core::execution::ledger()
            .reconcile_signature(signature, filled, detail)
            .await
        {
            info!(
                intent = %rec.intent_id,
                %signature,
                landed = filled,
                state = %rec.state,
                "execution intent reconciled against the chain"
            );
        }
        // Prefer the OMS mirror (keeps memory + DB in sync); fall back to a
        // direct DB update when the order was evicted from the mirror.
        if let Some(mgr) = self.state.orders() {
            let order = match OrderRepo::new(self.db.clone())
                .get_by_signature(signature)
                .await
            {
                Ok(Some(o)) => Some(o),
                _ => mgr
                    .list(1000)
                    .await
                    .into_iter()
                    .find(|o| o.signature.as_deref() == Some(signature)),
            };
            if let Some(order) = order {
                if !order.status.is_terminal() {
                    let target = if filled {
                        OrderStatus::Filled
                    } else {
                        OrderStatus::Failed
                    };
                    let res = mgr.transition(&order.id, target, Some(detail)).await;
                    if res.is_err() {
                        // Unknown → target is legal; anything else means the
                        // mirror disagrees with the DB — reconcile by upsert.
                        let _ = mgr
                            .transition(&order.id, OrderStatus::Unknown, Some("recon"))
                            .await;
                        let _ = mgr.transition(&order.id, target, Some(detail)).await;
                    }
                    info!(
                        order = %order.id,
                        %signature,
                        status = target.as_str(),
                        "order reconciled against the chain"
                    );
                }
            }
        }
    }

    /// Operator-visible alert for divergences that must not auto-correct.
    async fn flag_divergence(&self, action: &str, reason: &str, detail: serde_json::Value) {
        warn!(%reason, "RECONCILIATION DIVERGENCE");
        if let Err(e) = RiskEventRepo::new(self.db.clone())
            .append("system", action, None, reason, &detail)
            .await
        {
            debug!(error = %e, "risk-event persistence failed");
        }
        self.state
            .events
            .publish(bot_core::events::AppEvent::Error {
                ts: chrono::Utc::now(),
                module: None,
                message: reason.to_string(),
                fatal: false,
            });
    }
}

/// Database-backed [`IntentSink`] (§I crash point C). Write failures are
/// logged and metered but NEVER fail the trade: the journal is defence in
/// depth — a failed journal write degrades to the pre-journal behaviour.
pub struct DbIntentSink {
    db: Arc<Database>,
}

impl DbIntentSink {
    pub fn new(db: Arc<Database>) -> Self {
        DbIntentSink { db }
    }

    fn meter_error(op: &str) {
        metrics::global()
            .counter(
                "bot_intent_journal_errors_total",
                "Intent journal writes that failed (journaling is best-effort defence in depth).",
                &[("op", op)],
            )
            .inc();
    }
}

#[async_trait]
impl IntentSink for DbIntentSink {
    async fn record(&self, rec: IntentRecord) {
        if let Err(e) = IntentRepo::new(self.db.clone()).record(&rec).await {
            warn!(intent = %rec.intent_id, error = %e, "intent journal record failed");
            Self::meter_error("record");
        }
    }

    async fn link(&self, intent_id: &str, signature: &str) {
        if let Err(e) = IntentRepo::new(self.db.clone())
            .link(intent_id, signature)
            .await
        {
            warn!(%intent_id, error = %e, "intent journal link failed");
            Self::meter_error("link");
        }
    }

    async fn abandon(&self, intent_id: &str) {
        if let Err(e) = IntentRepo::new(self.db.clone()).abandon(intent_id).await {
            warn!(%intent_id, error = %e, "intent journal abandon failed");
            Self::meter_error("abandon");
        }
    }
}

/// Database-backed copy journal (TASK 3 §09, migration 0013). Same contract
/// as [`DbIntentSink`]: writes are best effort (logged + metered, never fail
/// the trade), reads answer `None` when the backend is unavailable so the
/// copy engine degrades explicitly instead of acting on an empty answer.
pub struct DbCopyStore {
    db: Arc<Database>,
}

impl DbCopyStore {
    pub fn new(db: Arc<Database>) -> Self {
        DbCopyStore { db }
    }

    fn repo(&self) -> bot_core::db::copy::CopyRepo {
        bot_core::db::copy::CopyRepo::new(self.db.clone())
    }

    fn meter_error(op: &str) {
        metrics::global()
            .counter(
                "copy_journal_errors_total",
                "Copy journal (leaders/events/links) writes that failed.",
                &[("op", op)],
            )
            .inc();
    }
}

#[async_trait]
impl module_copy::recovery::CopyStore for DbCopyStore {
    async fn upsert_leader(&self, rec: bot_core::db::copy::LeaderRecord) -> bool {
        match self.repo().upsert_leader(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(leader = %rec.address, error = %e, "copy leader upsert failed");
                Self::meter_error("upsert_leader");
                false
            }
        }
    }

    async fn append_leader_event(&self, rec: bot_core::db::copy::LeaderEventRecord) -> bool {
        match self.repo().append_leader_event(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(leader = %rec.address, event = %rec.event, error = %e, "copy leader event append failed");
                Self::meter_error("append_leader_event");
                false
            }
        }
    }

    async fn load_leaders(&self) -> Option<Vec<bot_core::db::copy::LeaderRecord>> {
        match self.repo().list_leaders().await {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, "copy leader load failed");
                Self::meter_error("load_leaders");
                None
            }
        }
    }

    async fn record_event(&self, rec: bot_core::db::copy::CopyEventRecord) -> bool {
        match self.repo().record_event(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(event = %rec.event_id, error = %e, "copy event journal failed");
                Self::meter_error("record_event");
                false
            }
        }
    }

    async fn events_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Option<Vec<bot_core::db::copy::CopyEventRecord>> {
        match self
            .repo()
            .events_since(since, limit.min(i64::MAX as usize) as i64)
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, "copy event journal read failed");
                Self::meter_error("events_since");
                None
            }
        }
    }

    async fn upsert_link(&self, rec: bot_core::db::copy::CopyLinkRecord) -> bool {
        match self.repo().upsert_link(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(position = %rec.position_id, error = %e, "copy link upsert failed");
                Self::meter_error("upsert_link");
                false
            }
        }
    }

    async fn open_links(&self) -> Option<Vec<bot_core::db::copy::CopyLinkRecord>> {
        match self.repo().open_links().await {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, "copy link read failed");
                Self::meter_error("open_links");
                None
            }
        }
    }

    async fn close_link(
        &self,
        position_id: &str,
        status: &str,
        exit_event_id: Option<&str>,
        note: Option<&str>,
    ) -> bool {
        match self
            .repo()
            .close_link(position_id, status, exit_event_id, note)
            .await
        {
            Ok(changed) => changed,
            Err(e) => {
                warn!(position = %position_id, error = %e, "copy link close failed");
                Self::meter_error("close_link");
                false
            }
        }
    }
}

/// Database-backed Polymarket journal (TASK 4, migration 0014). Same
/// contract as [`DbCopyStore`]: writes are best effort (logged + metered,
/// never fail the order path), reads answer `None` when the backend is
/// unavailable so the engine degrades explicitly ("journal unavailable")
/// instead of treating an empty answer as "no open orders".
pub struct DbPolyStore {
    db: Arc<Database>,
}

impl DbPolyStore {
    pub fn new(db: Arc<Database>) -> Self {
        DbPolyStore { db }
    }

    fn repo(&self) -> bot_core::db::polymarket::PolyRepo {
        bot_core::db::polymarket::PolyRepo::new(self.db.clone())
    }

    fn meter_error(op: &str) {
        metrics::global()
            .counter(
                "poly_journal_errors_total",
                "Polymarket journal writes/reads that failed.",
                &[("op", op)],
            )
            .inc();
    }
}

#[async_trait]
impl module_polymarket::store::PolyStore for DbPolyStore {
    async fn record_signal(&self, rec: bot_core::db::polymarket::PolySignalRecord) -> bool {
        match self.repo().record_signal(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(signal = %rec.signal_id, error = %e, "poly signal journal failed");
                Self::meter_error("record_signal");
                false
            }
        }
    }

    async fn upsert_order(&self, rec: bot_core::db::polymarket::PolyOrderRecord) -> bool {
        match self.repo().upsert_order(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(order = %rec.venue_order_id, error = %e, "poly order journal failed");
                Self::meter_error("upsert_order");
                false
            }
        }
    }

    async fn open_orders(&self) -> Option<Vec<bot_core::db::polymarket::PolyOrderRecord>> {
        match self.repo().open_orders().await {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, "poly open-order journal read failed");
                Self::meter_error("open_orders");
                None
            }
        }
    }

    async fn record_fill(&self, rec: bot_core::db::polymarket::PolyFillRecord) -> Option<bool> {
        match self.repo().record_fill(&rec).await {
            Ok(inserted) => Some(inserted),
            Err(e) => {
                warn!(fill = %rec.fill_id, error = %e, "poly fill journal failed");
                Self::meter_error("record_fill");
                None
            }
        }
    }

    async fn append_finding(&self, rec: bot_core::db::polymarket::PolyReconFindingRecord) -> bool {
        match self.repo().append_finding(&rec).await {
            Ok(()) => true,
            Err(e) => {
                warn!(kind = %rec.kind, error = %e, "poly recon finding journal failed");
                Self::meter_error("append_finding");
                false
            }
        }
    }
}

/// Resolves `intent` claims. A pending orphan means the process died (or the
/// link write failed) between broadcast and any signature being journaled:
/// the outcome is AMBIGUOUS — there is no signature to look up, so the claim
/// retries (giving the link a chance to land) and then parks for operators.
/// The orphan's symbol stays entry-gated meanwhile (§H). Never resubmitted.
pub struct IntentTruth {
    db: Arc<Database>,
}

impl IntentTruth {
    pub fn new(db: Arc<Database>) -> Self {
        IntentTruth { db }
    }
}

#[async_trait]
impl TruthSource for IntentTruth {
    fn kind(&self) -> &str {
        "intent"
    }

    async fn resolve(&self, subject: &str) -> ReconVerdict {
        match IntentRepo::new(self.db.clone()).get(subject).await {
            Err(e) => {
                meter_read_error("postgres");
                ReconVerdict::Retry {
                    reason: format!("intent journal unreadable: {e}"),
                }
            }
            Ok(None) => {
                meter_outcome("intent", &ReconOutcome::MissingTransaction);
                ReconVerdict::GiveUp {
                    reason: format!("intent {subject} missing from the journal"),
                }
            }
            Ok(Some(rec)) => match rec.status.as_str() {
                "submitted" => {
                    meter_outcome("intent", &ReconOutcome::InSync);
                    ReconVerdict::Resolved {
                        status: "submitted".into(),
                    }
                }
                "abandoned" => {
                    meter_outcome("intent", &ReconOutcome::InSync);
                    ReconVerdict::Resolved {
                        status: "abandoned".into(),
                    }
                }
                other => {
                    meter_outcome("intent", &ReconOutcome::UnknownExecution);
                    ReconVerdict::Retry {
                        reason: format!(
                            "intent still '{other}' — no signature journaled; outcome ambiguous,                              symbol stays gated, never resubmitted"
                        ),
                    }
                }
            },
        }
    }
}

#[async_trait]
impl TruthSource for SolanaTxTruth {
    fn kind(&self) -> &str {
        "transaction"
    }

    async fn resolve(&self, subject: &str) -> ReconVerdict {
        let Ok(signature) = Signature::from_str(subject) else {
            return ReconVerdict::GiveUp {
                reason: "subject is not a valid signature".into(),
            };
        };
        let tx_repo = TransactionRepo::new(self.db.clone());

        // ---- external observation -----------------------------------------
        let probe = self.probe(&signature).await;
        let external = match &probe {
            Ok(ConfirmOutcome::Confirmed { .. }) => {
                ExternalState::Observed(ExternalExecutionState::Succeeded)
            }
            Ok(ConfirmOutcome::Failed { .. }) => {
                ExternalState::Observed(ExternalExecutionState::FailedOnExternal)
            }
            // The blockhash outlived its validity window with no trace of the
            // signature: the transaction can never land — a definitive
            // "will not fill", handled exactly like a chain failure.
            Ok(ConfirmOutcome::Expired { .. }) => {
                ExternalState::Observed(ExternalExecutionState::FailedOnExternal)
            }
            // Not visible yet: could be pending OR never-landed; the worker's
            // attempt budget decides when "still nothing" becomes a park.
            Ok(ConfirmOutcome::Timeout) => ExternalState::Observed(ExternalExecutionState::Pending),
            Err(reason) => {
                meter_read_error("solana_rpc");
                ExternalState::Unavailable {
                    reason: reason.clone(),
                }
            }
        };

        let local = self.local_state(subject).await;
        let outcome = classify_execution(local, external.clone());
        meter_outcome("transaction", &outcome);

        // ---- act on the external truth -------------------------------------
        match external {
            ExternalState::Observed(ExternalExecutionState::Succeeded) => {
                let (slot, detail) = match &probe {
                    Ok(ConfirmOutcome::Confirmed { slot, fee, .. }) => (
                        Some(*slot as i64),
                        format!("confirmed slot={slot} fee={fee}"),
                    ),
                    _ => (None, "confirmed on chain".into()),
                };
                if let Err(e) = tx_repo.set_status(subject, "confirmed", slot, None).await {
                    debug!(error = %e, "tx status persistence failed");
                }
                self.finish_order(subject, true, &detail).await;
                ReconVerdict::Resolved { status: detail }
            }
            ExternalState::Observed(ExternalExecutionState::FailedOnExternal) => {
                if local == LocalExecutionState::RecordedConfirmed {
                    // Books say success, chain says failure: fill effects were
                    // already applied — mechanical correction is unsafe (§Y).
                    let reason = format!(
                        "transaction {subject} recorded CONFIRMED locally but failed on chain"
                    );
                    self.flag_divergence(
                        "recon_conflict",
                        &reason,
                        serde_json::json!({ "signature": subject }),
                    )
                    .await;
                    return ReconVerdict::GiveUp { reason };
                }
                let err = match &probe {
                    Ok(ConfirmOutcome::Failed { error, .. }) => error.clone(),
                    Ok(ConfirmOutcome::Expired {
                        last_valid_block_height,
                        block_height,
                    }) => format!(
                        "blockhash expired: block height {block_height} passed last valid \
                         {last_valid_block_height} with no trace of the signature"
                    ),
                    _ => "failed on chain".into(),
                };
                if let Err(e) = tx_repo
                    .set_status(subject, "failed", None, Some(&err))
                    .await
                {
                    debug!(error = %e, "tx status persistence failed");
                }
                self.finish_order(subject, false, &format!("failed on chain: {err}"))
                    .await;
                ReconVerdict::Resolved {
                    status: "failed".into(),
                }
            }
            ExternalState::Observed(ExternalExecutionState::Pending) => ReconVerdict::Retry {
                reason: "signature not resolved on chain yet".into(),
            },
            ExternalState::Observed(ExternalExecutionState::NotFound) => ReconVerdict::Retry {
                reason: "no external trace yet".into(),
            },
            ExternalState::Unavailable { reason } => ReconVerdict::Retry {
                reason: format!("external state unavailable: {reason}"),
            },
        }
    }
}

/// Resolve Polymarket CLOB order ids against the provider API (§Q).
pub struct PolymarketOrderTruth {
    bot: Arc<tokio::sync::Mutex<module_polymarket::PolyBot>>,
    state: Shared,
    db: Arc<Database>,
}

impl PolymarketOrderTruth {
    pub async fn new(state: Shared, db: Arc<Database>) -> Option<Self> {
        match module_polymarket::PolyBot::new(state.clone()).await {
            Ok(bot) => Some(PolymarketOrderTruth {
                bot: Arc::new(tokio::sync::Mutex::new(bot)),
                state,
                db,
            }),
            Err(e) => {
                warn!(error = %e, "polymarket recon source unavailable");
                None
            }
        }
    }

    /// The position truth source for the same venue, sharing this source's
    /// read-only `PolyBot` handle (one signer / API-key derivation, one CTF
    /// reader) instead of constructing a second one.
    pub fn position_truth(&self) -> PolymarketPositionTruth {
        PolymarketPositionTruth {
            bot: Arc::clone(&self.bot),
            state: self.state.clone(),
            db: self.db.clone(),
        }
    }

    async fn finish_order(&self, external_id: &str, status: OrderStatus, detail: &str) {
        let Some(mgr) = self.state.orders() else {
            return;
        };
        let orders = mgr.list(1000).await;
        if let Some(order) = orders
            .iter()
            .find(|o| o.external_id.as_deref() == Some(external_id))
        {
            if !order.status.is_terminal() {
                if let Err(e) = mgr.transition(&order.id, status, Some(detail)).await {
                    debug!(error = %e, order = %order.id, "poly recon transition failed");
                }
            }
        }
    }
}

#[async_trait]
impl TruthSource for PolymarketOrderTruth {
    fn kind(&self) -> &str {
        "polymarket_order"
    }

    async fn resolve(&self, subject: &str) -> ReconVerdict {
        let bot = self.bot.lock().await;
        let response = match bot.order_status(subject).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                // No credentials: the venue cannot be read — never a verdict.
                meter_read_error("polymarket_clob");
                return ReconVerdict::Retry {
                    reason: "no CLOB credentials available".into(),
                };
            }
            Err(e) => {
                let msg = e.to_string();
                // The provider answers 404-ish errors for unknown orders.
                if msg.contains("404") || msg.to_lowercase().contains("not found") {
                    meter_outcome("polymarket_order", &ReconOutcome::MissingTransaction);
                    return ReconVerdict::GiveUp {
                        reason: format!("provider does not know this order: {msg}"),
                    };
                }
                meter_read_error("polymarket_clob");
                return ReconVerdict::Retry { reason: msg };
            }
        };
        drop(bot);

        // CLOB order status strings: live/unmatched (resting), matched
        // (filled), canceled/cancelled, expired.
        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (verdict, target) = match status.as_str() {
            "matched" | "filled" => (
                ReconVerdict::Resolved {
                    status: "matched".into(),
                },
                Some(OrderStatus::Filled),
            ),
            "canceled" | "cancelled" => (
                ReconVerdict::Resolved {
                    status: "cancelled".into(),
                },
                Some(OrderStatus::Cancelled),
            ),
            "expired" => (
                ReconVerdict::Resolved {
                    status: "expired".into(),
                },
                Some(OrderStatus::Expired),
            ),
            "live" | "unmatched" | "delayed" | "" => (
                // Resting on the book: the provider state is KNOWN; the
                // order stays live and the module keeps managing it.
                ReconVerdict::Resolved {
                    status: format!("resting:{status}"),
                },
                Some(OrderStatus::Accepted),
            ),
            other => (
                ReconVerdict::Retry {
                    reason: format!("unrecognised provider status '{other}'"),
                },
                None,
            ),
        };
        // §Q/§D: the venue says MATCHED — verify on-chain settlement via the
        // CTF (ERC-1155) balance and surface missed fill events (order Filled
        // but no local position). Evidence-only: positions are created by the
        // fill path from authoritative fill data, never fabricated here (§Y).
        if matches!(target, Some(OrderStatus::Filled)) {
            let asset_id = response
                .get("asset_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !asset_id.is_empty()
                && self
                    .state
                    .find_open(bot_core::models::BotModule::Polymarket, &asset_id)
                    .await
                    .is_none()
            {
                let bot = self.bot.lock().await;
                match bot.ctf_balance(&asset_id).await {
                    Ok(Some(bal)) if bal > 0 => {
                        let reason = format!(
                            "polymarket order {subject} matched on the venue and {bal} outcome                              tokens are held on-chain, but no local position exists for                              {asset_id} — fill event missed; position NOT auto-created                              (cost basis must come from authoritative fills)"
                        );
                        warn!(%reason, "POLYMARKET SETTLEMENT DIVERGENCE");
                        if let Err(e) = RiskEventRepo::new(self.db.clone())
                            .append(
                                "system",
                                "poly_settled_no_position",
                                Some(subject),
                                &reason,
                                &serde_json::json!({
                                    "asset_id": asset_id,
                                    "ctf_balance": bal.to_string(),
                                }),
                            )
                            .await
                        {
                            debug!(error = %e, "risk-event persistence failed");
                        }
                        self.state
                            .events
                            .publish(bot_core::events::AppEvent::Error {
                                ts: chrono::Utc::now(),
                                module: None,
                                message: reason,
                                fatal: false,
                            });
                    }
                    // Matched but tokens not held: sold/transferred after the
                    // match, or unsettled — the ORDER state itself is resolved
                    // either way; no local position means nothing to correct.
                    Ok(Some(_)) => {}
                    // CTF reader not configured: venue API stays the truth.
                    Ok(None) => {}
                    // Could not read ≠ no balance (§O): inconclusive, metered.
                    Err(e) => {
                        meter_read_error("polygon_ctf");
                        debug!(error = %e, "CTF balance read failed (inconclusive)");
                    }
                }
            }
        }

        let poly_outcome = match &verdict {
            ReconVerdict::Resolved { .. } => ReconOutcome::InSync,
            ReconVerdict::Retry { .. } => ReconOutcome::ExternalStateUnavailable {
                reason: String::new(),
            },
            ReconVerdict::GiveUp { .. } => ReconOutcome::MissingTransaction,
        };
        meter_outcome("polymarket_order", &poly_outcome);
        if let Some(target) = target {
            self.finish_order(subject, target, &format!("provider status: {status}"))
                .await;
        }
        verdict
    }
}

/// Compare live-mode Solana positions against the wallet's AGGREGATED
/// on-chain token balance through the reconciliation engine (§J).
///
/// Paper/simulate positions have no chain truth and resolve immediately.
/// Divergences are FLAGGED (risk event + system-visible warning), not
/// auto-corrected — with the deterministic, fill-justified exceptions of
/// [`settle_position_outcome`] (the one correction policy shared with
/// [`PolymarketPositionTruth`]): e.g. when the chain says ZERO, the position
/// is open locally, no execution is in flight, and the persisted fill history
/// contains a confirmed exit, the position is closed and its PnL RECOMPUTED
/// from those fills (§K — never an arbitrary overwrite). Anything
/// authoritative data cannot explain is parked for operators.
pub struct SolanaPositionTruth {
    rpc: Rpc,
    state: Shared,
    db: Arc<Database>,
    wallet: String,
}

impl SolanaPositionTruth {
    pub fn new(rpc: Rpc, state: Shared, db: Arc<Database>, wallet: String) -> Self {
        SolanaPositionTruth {
            rpc,
            state,
            db,
            wallet,
        }
    }
}

#[async_trait]
impl TruthSource for SolanaPositionTruth {
    fn kind(&self) -> &str {
        "position"
    }

    async fn resolve(&self, subject: &str) -> ReconVerdict {
        let position = match self.state.position(subject).await {
            Some(p) => p,
            None => match PositionRepo::new(self.db.clone()).get(subject).await {
                Ok(Some(p)) => p,
                _ => {
                    return ReconVerdict::GiveUp {
                        reason: "position does not exist".into(),
                    }
                }
            },
        };
        if position.mode != ExecutionMode::Live {
            return ReconVerdict::Resolved {
                status: format!("{} positions have no chain truth", position.mode.as_str()),
            };
        }
        let Ok(mint) = solana_sdk::pubkey::Pubkey::from_str(&position.symbol) else {
            return ReconVerdict::GiveUp {
                reason: "symbol is not a mint address".into(),
            };
        };
        let Ok(owner) = solana_sdk::pubkey::Pubkey::from_str(&self.wallet) else {
            return ReconVerdict::Retry {
                reason: "wallet pubkey unavailable".into(),
            };
        };

        // ---- external observation (§D/§O: aggregated, identity-validated;
        // Err = unreadable, NOT zero) ----------------------------------------
        let external = match token_balances_for_owner(&self.rpc, &owner, &mint).await {
            Ok(r) => ExternalState::Observed(BalanceObservation {
                raw_total: r.raw_total,
                ui_total: r.ui_total,
                decimals: r.decimals,
                accounts: r.accounts,
            }),
            Err(e) => {
                meter_read_error("solana_position");
                ExternalState::Unavailable {
                    reason: e.to_string(),
                }
            }
        };

        // ---- local snapshot --------------------------------------------------
        // Money in flight for this position? The entry signature's claim is
        // the durable marker; while it is active, divergences are expected.
        let unresolved = match &position.entry_signature {
            Some(sig) if !sig.is_empty() => ReconRepo::new(self.db.clone())
                .is_active("transaction", sig)
                .await
                .unwrap_or(false),
            _ => false,
        };
        let fills = fills_for_position(&self.db, subject).await;
        let snapshot = PositionSnapshot {
            id: subject.to_string(),
            qty: position.qty,
            open: matches!(
                position.status,
                PositionStatus::Open | PositionStatus::Closing
            ),
            has_unresolved_execution: unresolved,
            fills: fills.clone(),
        };

        let outcome = compare_position(&snapshot, &external, QuantityTolerance::default());
        meter_outcome("position", &outcome);
        settle_position_outcome(&self.state, &self.db, subject, &position, outcome, &fills).await
    }
}

/// Persisted, confirmed fill history → engine [`FillRecord`]s.
///
/// Trade conventions (see module record paths): buys carry the quote
/// spent in `amount_in` (fees included — it is what actually left the
/// wallet) and the base qty in `amount_out`; sells carry the base qty in
/// `amount_in` and the quote received in `amount_out`, with any separate
/// venue `fee` deducted from proceeds. Shared by the Solana and the Polymarket
/// position truth sources: the fill ledger has the same shape on both venues.
async fn fills_for_position(db: &Arc<Database>, position_id: &str) -> Vec<FillRecord> {
    match TradeRepo::new(db.clone())
        .list_for_position(position_id)
        .await
    {
        Ok(trades) => trades
            .into_iter()
            .map(|t| {
                if t.is_buy() {
                    FillRecord {
                        side: "buy".into(),
                        qty: t.amount_out,
                        quote: t.amount_in + t.fee,
                    }
                } else {
                    FillRecord {
                        side: "sell".into(),
                        qty: t.amount_in,
                        quote: t.amount_out - t.fee,
                    }
                }
            })
            .collect(),
        Err(e) => {
            debug!(error = %e, "fill history read failed");
            Vec::new()
        }
    }
}

/// Operator-visible alert (risk event + event bus) for divergences.
async fn flag_position(
    state: &Shared,
    db: &Arc<Database>,
    action: &str,
    reason: &str,
    detail: serde_json::Value,
) {
    warn!(%reason, "POSITION RECONCILIATION");
    if let Err(e) = RiskEventRepo::new(db.clone())
        .append("system", action, None, reason, &detail)
        .await
    {
        debug!(error = %e, "risk-event persistence failed");
    }
    state.events.publish(bot_core::events::AppEvent::Error {
        ts: chrono::Utc::now(),
        module: None,
        message: reason.to_string(),
        fatal: false,
    });
}

/// Act on a typed position-reconciliation outcome (§J/§K) — ONE policy for
/// every venue. Divergences are FLAGGED (risk event + system-visible
/// warning), not auto-corrected, with two deterministic, fill-justified
/// exceptions: (1) the chain says ZERO, the position is open locally, no
/// execution is in flight and the persisted fill history contains a
/// confirmed exit → the position is closed and its PnL RECOMPUTED from those
/// fills; (2) the on-chain quantity is reproduced by the fill history within
/// tolerance → the book adopts it. Anything authoritative data cannot explain
/// is parked for operators (§Y).
async fn settle_position_outcome(
    state: &Shared,
    db: &Arc<Database>,
    subject: &str,
    position: &bot_core::models::Position,
    outcome: ReconOutcome,
    fills: &[FillRecord],
) -> ReconVerdict {
    // ---- act on the typed outcome ----------------------------------------
    match outcome {
        ReconOutcome::InSync => ReconVerdict::Resolved {
            status: "verified".into(),
        },
        ReconOutcome::UnknownExecution
        | ReconOutcome::StaleLocalState
        | ReconOutcome::ExternalStateUnavailable { .. } => ReconVerdict::Retry {
            reason: format!("inconclusive: {}", outcome.as_str()),
        },
        ReconOutcome::MissingPosition => {
            // Chain says zero, book says open, nothing in flight, and a
            // confirmed exit fill exists → close the position and
            // RECOMPUTE its economics from the authoritative fills (§K).
            let recon = reconstruct_pnl(fills);
            let mut corrected = position.clone();
            corrected.qty = recon.open_qty.max(0.0);
            corrected.cost_basis = recon.buy_cost;
            corrected.realized_quote = recon.sell_proceeds;
            // Keep the in-memory book consistent before close_position
            // books PnL from these fields (realised = realized - cost).
            state.upsert_position(corrected.clone()).await;
            let closed = state
                .close_position(
                    subject,
                    PositionStatus::Closed,
                    "reconciled: zero on-chain with confirmed exit fill",
                )
                .await;
            let final_pos = closed.unwrap_or(corrected);
            if let Err(e) = PositionRepo::new(db.clone()).upsert(&final_pos).await {
                debug!(error = %e, "corrected position persistence failed");
            }
            let reason = format!(
                "position {subject} corrected: on-chain balance zero, closed from fill history \
                 (realized {} quote over {} fills)",
                recon.realized,
                fills.len()
            );
            flag_position(
                state,
                db,
                "recon_correction",
                &reason,
                serde_json::json!({
                    "position_id": subject,
                    "realized": recon.realized,
                    "open_qty": recon.open_qty,
                    "fills": fills.len(),
                }),
            )
            .await;
            ReconVerdict::Resolved {
                status: "corrected-closed".into(),
            }
        }
        ReconOutcome::LocalAhead { local, external }
        | ReconOutcome::ExternalAhead { local, external }
        | ReconOutcome::QuantityMismatch { local, external }
        | ReconOutcome::BalanceMismatch {
            expected: local,
            observed: external,
        } => {
            // Widened drift correction (§J): adopt the on-chain quantity
            // ONLY when the authoritative FILL history independently
            // reproduces it within the same tolerance — a deterministic,
            // fill-justified correction (book write was lost, fills are
            // durable). Anything the fills cannot explain is flagged for
            // operators, never silently rewritten (§Y).
            let recon = reconstruct_pnl(fills);
            let tol = QuantityTolerance::default();
            let justified = !fills.is_empty()
                && (recon.open_qty - external).abs() <= tol.dust.max(external.abs() * tol.relative);
            if justified {
                let mut corrected = position.clone();
                corrected.qty = recon.open_qty.max(0.0);
                corrected.cost_basis = recon.buy_cost;
                corrected.realized_quote = recon.sell_proceeds;
                corrected.updated_at = chrono::Utc::now();
                state.upsert_position(corrected.clone()).await;
                if let Err(e) = PositionRepo::new(db.clone()).upsert(&corrected).await {
                    debug!(error = %e, "corrected position persistence failed");
                }
                let reason = format!(
                    "position {subject} corrected to on-chain {external} (book said {local}):                          fill history reproduces the observed balance ({} fills, open_qty {})",
                    fills.len(),
                    recon.open_qty
                );
                flag_position(
                    state,
                    db,
                    "recon_correction",
                    &reason,
                    serde_json::json!({
                        "position_id": subject,
                        "outcome": outcome.as_str(),
                        "book_qty": local,
                        "onchain_qty": external,
                        "fills_open_qty": recon.open_qty,
                        "fills": fills.len(),
                    }),
                )
                .await;
                ReconVerdict::Resolved {
                    status: "corrected-adopted".into(),
                }
            } else {
                // Drift beyond tolerance and NOT fill-justified: flag,
                // never silently rewrite quantities (deliberate policy).
                let reason = format!(
                    "position {subject} drift ({}): book={local} on-chain={external}",
                    outcome.as_str()
                );
                flag_position(
                    state,
                    db,
                    "drift_flag",
                    &reason,
                    serde_json::json!({
                        "position_id": subject,
                        "outcome": outcome.as_str(),
                        "book_qty": local,
                        "onchain_qty": external,
                    }),
                )
                .await;
                ReconVerdict::Resolved {
                    status: "drift-flagged".into(),
                }
            }
        }
        ReconOutcome::UnexpectedPosition { external } => {
            let reason = format!(
                "unexpected on-chain balance {external} with no open local position {subject}"
            );
            flag_position(
                state,
                db,
                "unexpected_position",
                &reason,
                serde_json::json!({ "position_id": subject, "onchain_qty": external }),
            )
            .await;
            ReconVerdict::GiveUp { reason }
        }
        ReconOutcome::RecoveryRequired { reason } => {
            let full = format!("position {subject}: {reason}");
            flag_position(
                state,
                db,
                "recon_recovery_required",
                &full,
                serde_json::json!({ "position_id": subject }),
            )
            .await;
            ReconVerdict::GiveUp { reason: full }
        }
        // Not reachable from compare_position, but the match stays total:
        ReconOutcome::MissingTransaction | ReconOutcome::DuplicateExecution => {
            ReconVerdict::Retry {
                reason: format!("unexpected outcome {}", outcome.as_str()),
            }
        }
    }
}

/// Compare live-mode Polymarket positions against the funder's SETTLED
/// outcome-token balance (CTF ERC-1155 `balanceOf`, `module_polymarket::ctf`)
/// through the same reconciliation engine and the same correction policy as
/// Solana positions (§J/§K). Before this source existed, Polymarket positions
/// were excluded from the periodic on-chain re-verification
/// (`position_recheck_interval_secs`) — the CLOB order status is the truth
/// for ORDERS (`polymarket_order`); the on-chain balance is the truth for what
/// is actually HELD.
///
/// A non-terminal OMS order on the same token marks the snapshot as
/// "execution in flight", so expected divergences (resting or partially
/// matched orders) never trigger a correction. §O: a failed read is
/// `Unavailable`, never zero. The CTF reader is optional
/// (`[polymarket].ctf_rpc_url`): without it the claim resolves as
/// unverifiable — and the recheck loop does not enqueue it in the first place.
pub struct PolymarketPositionTruth {
    bot: Arc<tokio::sync::Mutex<module_polymarket::PolyBot>>,
    state: Shared,
    db: Arc<Database>,
}

/// Outcome tokens settle at the collateral's scale (six decimals); one
/// ERC-1155 balance is one contributing "account". Values beyond `u64` keep
/// the exact `ui_total` and saturate the raw counter instead of wrapping.
pub(crate) fn ctf_balance_observation(raw: u128) -> BalanceObservation {
    const DECIMALS: u8 = 6;
    BalanceObservation {
        raw_total: u64::try_from(raw).unwrap_or(u64::MAX),
        ui_total: raw as f64 / 10f64.powi(i32::from(DECIMALS)),
        decimals: DECIMALS,
        accounts: 1,
    }
}

#[async_trait]
impl TruthSource for PolymarketPositionTruth {
    fn kind(&self) -> &str {
        "polymarket_position"
    }

    async fn resolve(&self, subject: &str) -> ReconVerdict {
        let position = match self.state.position(subject).await {
            Some(p) => p,
            None => match PositionRepo::new(self.db.clone()).get(subject).await {
                Ok(Some(p)) => p,
                _ => {
                    return ReconVerdict::GiveUp {
                        reason: "position does not exist".into(),
                    }
                }
            },
        };
        if position.mode != ExecutionMode::Live {
            return ReconVerdict::Resolved {
                status: format!("{} positions have no chain truth", position.mode.as_str()),
            };
        }
        if !matches!(position.venue, bot_core::models::Venue::PolymarketClob) {
            return ReconVerdict::GiveUp {
                reason: format!("position {subject} is not a Polymarket position"),
            };
        }

        // ---- external observation (§O: Err = unreadable, NOT zero) ----------
        let observed = {
            let bot = self.bot.lock().await;
            bot.ctf_balance(&position.symbol).await
        };
        let external = match observed {
            Ok(Some(raw)) => ExternalState::Observed(ctf_balance_observation(raw)),
            Ok(None) => {
                meter_outcome(
                    "polymarket_position",
                    &ReconOutcome::ExternalStateUnavailable {
                        reason: String::new(),
                    },
                );
                return ReconVerdict::Resolved {
                    status: "unverifiable: CTF reader not configured ([polymarket].ctf_rpc_url)"
                        .into(),
                };
            }
            Err(e) => {
                meter_read_error("polygon_ctf");
                ExternalState::Unavailable {
                    reason: e.to_string(),
                }
            }
        };

        // ---- local snapshot --------------------------------------------------
        // Money in flight for this token? Any non-terminal OMS order of the
        // Polymarket module on the same outcome token (resting, partially
        // matched, or ambiguous) — while one exists, divergences are expected.
        let unresolved = match self.state.orders() {
            Some(mgr) => mgr.list(1000).await.iter().any(|o| {
                o.module == bot_core::models::BotModule::Polymarket
                    && o.symbol == position.symbol
                    && !o.status.is_terminal()
            }),
            None => false,
        };
        let fills = fills_for_position(&self.db, subject).await;
        let snapshot = PositionSnapshot {
            id: subject.to_string(),
            qty: position.qty,
            open: matches!(
                position.status,
                PositionStatus::Open | PositionStatus::Closing
            ),
            has_unresolved_execution: unresolved,
            fills: fills.clone(),
        };

        let outcome = compare_position(&snapshot, &external, QuantityTolerance::default());
        meter_outcome("polymarket_position", &outcome);
        settle_position_outcome(&self.state, &self.db, subject, &position, outcome, &fills).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_parse_rejects_garbage() {
        assert!(Signature::from_str("not-a-signature").is_err());
        // A real (all-ones) base58 signature parses.
        let sig = Signature::from([1u8; 64]);
        assert_eq!(Signature::from_str(&sig.to_string()).unwrap(), sig);
    }

    #[test]
    fn ctf_observation_uses_six_decimals_and_never_wraps() {
        let o = ctf_balance_observation(1_500_000);
        assert_eq!(o.raw_total, 1_500_000);
        assert!((o.ui_total - 1.5).abs() < 1e-12);
        assert_eq!(o.decimals, 6);
        assert_eq!(o.accounts, 1);
        // Zero is a real observation (the chain answered), not "unavailable".
        let z = ctf_balance_observation(0);
        assert_eq!(z.raw_total, 0);
        assert_eq!(z.ui_total, 0.0);
        // A uint256 beyond u64 keeps the exact human quantity and saturates
        // the raw counter instead of silently wrapping to a small number.
        let big = ctf_balance_observation(u128::from(u64::MAX) + 10);
        assert_eq!(big.raw_total, u64::MAX);
        assert!(big.ui_total > 1.8e13);
    }

    #[test]
    fn fill_mapping_conventions_are_documented_and_pure() {
        // Buy: quote spent = amount_in (+fee); qty = amount_out.
        // Sell: qty = amount_in; net proceeds = amount_out - fee.
        // reconstruct_pnl over that mapping must reproduce hand-computed PnL:
        // bought 100 for 10 SOL (fee incl.), sold 100 for 12 SOL, fee 0.1 →
        // proceeds 11.9 → realized 1.9.
        let fills = vec![
            FillRecord {
                side: "buy".into(),
                qty: 100.0,
                quote: 10.0,
            },
            FillRecord {
                side: "sell".into(),
                qty: 100.0,
                quote: 12.0 - 0.1,
            },
        ];
        let r = reconstruct_pnl(&fills);
        assert!((r.realized - 1.9).abs() < 1e-12);
        assert_eq!(r.open_qty, 0.0);
    }
}
