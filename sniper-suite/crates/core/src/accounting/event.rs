//! Typed accounting events — the ONLY way a module (or an operator) can move
//! money through the global ledger (TASK 5 §2, §4, §11).
//!
//! A module never mutates ledger state: it builds an [`AccountingEvent`] that
//! names the source module, venue, wallet, strategy, asset, quantity, price,
//! fee, timestamp and the reference / correlation ids, and hands it to
//! [`crate::accounting::GlobalLedger::submit`]. The ledger derives the
//! deterministic [`AccountingEvent::event_id`] — the one idempotency
//! identity for financial events across fills, fees, settlements, recovery
//! replays and reconciliation adjustments — and books it exactly once.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{BotModule, ExecutionMode, Trade, Venue};

/// Prefix of every ledger event id.
pub const EVENT_ID_PREFIX: &str = "led_";

/// Kind of financial mutation. Closed vocabulary (bounded metric label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A confirmed venue fill (buy or sell) of one asset against its quote.
    Fill,
    /// A fee charged separately from any fill (network / venue / funding
    /// fee that reached the wallet as its own cash movement).
    Fee,
    /// Settlement of a position by the venue (redemption at resolution,
    /// expiry cash-out): the asset leaves, cash arrives.
    Settlement,
    /// External capital added to a wallet.
    Deposit,
    /// External capital removed from a wallet.
    Withdrawal,
    /// Cash moved between two wallets the suite controls.
    Transfer,
    /// Venue funding / rebate / interest adjustment to a wallet's cash.
    FundingAdjustment,
    /// An explicit, referenced correction (reconciliation adjustment). Never
    /// produced automatically — it always names the finding it answers.
    Correction,
}

impl EventKind {
    /// Every kind, in a stable order (metrics tables, docs, tests).
    pub const ALL: [EventKind; 8] = [
        EventKind::Fill,
        EventKind::Fee,
        EventKind::Settlement,
        EventKind::Deposit,
        EventKind::Withdrawal,
        EventKind::Transfer,
        EventKind::FundingAdjustment,
        EventKind::Correction,
    ];

    /// Stable lowercase label (metrics, journal, audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Fill => "fill",
            EventKind::Fee => "fee",
            EventKind::Settlement => "settlement",
            EventKind::Deposit => "deposit",
            EventKind::Withdrawal => "withdrawal",
            EventKind::Transfer => "transfer",
            EventKind::FundingAdjustment => "funding_adjustment",
            EventKind::Correction => "correction",
        }
    }

    /// Inverse of [`EventKind::as_str`] (durable rows, API input).
    pub fn parse(s: &str) -> Option<EventKind> {
        EventKind::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Kinds that change the quantity of an asset position.
    pub fn moves_inventory(&self) -> bool {
        matches!(
            self,
            EventKind::Fill | EventKind::Settlement | EventKind::Correction
        )
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Direction of an inventory-moving event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSide {
    /// Asset quantity increases, cash decreases.
    Buy,
    /// Asset quantity decreases, cash increases.
    Sell,
}

impl EventSide {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventSide::Buy => "buy",
            EventSide::Sell => "sell",
        }
    }

    /// Inverse of [`EventSide::as_str`].
    pub fn parse(s: &str) -> Option<EventSide> {
        match s {
            "buy" => Some(EventSide::Buy),
            "sell" => Some(EventSide::Sell),
            _ => None,
        }
    }
}

/// One financial mutation as reported by its source. Plain data: the ledger
/// validates it ([`AccountingEvent::validate`]) and derives everything else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountingEvent {
    /// What happened.
    pub kind: EventKind,
    /// Module that observed / caused it (`Telegram` = operator input).
    pub module: BotModule,
    /// Venue the money moved on (`Paper` for paper fills).
    pub venue: Venue,
    /// Our account: Solana pubkey, Polygon EOA, or an operator-named
    /// account for deposits / transfers.
    pub wallet: String,
    /// Strategy label the exposure is attributed to (`sniper`,
    /// `copy:<leader>`, the Polymarket strategy name, `operator`).
    pub strategy: String,
    /// Base asset: token mint / CLOB token id. For cash-only kinds
    /// (deposit, withdrawal, transfer, funding, fee) this is the cash asset.
    pub asset: String,
    /// Quote / cash asset the amounts below are denominated in (`SOL`,
    /// `USDC`).
    pub quote_asset: String,
    /// Buy / sell for inventory-moving kinds; `None` otherwise.
    pub side: Option<EventSide>,
    /// Base units moved (fills / settlements / corrections) or cash amount
    /// (deposit / withdrawal / transfer / funding / fee). Must be finite and
    /// `>= 0`.
    pub quantity: f64,
    /// Quote per unit where applicable.
    pub price: Option<f64>,
    /// Cash that actually moved in `quote_asset`, fee INCLUDED on the way
    /// out (buy: spent, sell: received net of the venue fee). Same
    /// convention as `Trade.amount_in` / `amount_out`.
    pub quote_amount: f64,
    /// Fee paid in `quote_asset` (`0` when not reported separately).
    pub fee: f64,
    /// Execution mode the event was produced under.
    pub mode: ExecutionMode,
    /// Source-defined identity of the underlying fact: transaction
    /// signature, venue trade id, deterministic paper fill id, deposit
    /// reference. Part of the idempotency identity — the same fact
    /// reported twice must carry the same reference.
    pub reference_id: String,
    /// OMS order id / intent id / claim id the event belongs to.
    pub correlation_id: Option<String>,
    /// Module position the event belongs to (aggregation join key).
    pub position_id: Option<String>,
    /// Module trade record id (`Trade.id`) the event books, when it books
    /// one — the reconciliation join key between trades and ledger events.
    pub trade_id: Option<String>,
    /// Transfers: the receiving wallet (`wallet` is the sending one).
    pub counterparty_wallet: Option<String>,
    /// When the fact happened (venue / chain time when known).
    pub ts: DateTime<Utc>,
    /// Free-form, single-line detail (audit / journal).
    pub detail: String,
}

impl AccountingEvent {
    /// Deterministic event id: a digest of the identity fields
    /// `(kind, module, venue, wallet, reference_id)`. Two submissions that
    /// describe the same fact produce the same id and the ledger books the
    /// second one as a duplicate. Amounts are deliberately NOT part of the
    /// identity: a replay that carries a different amount for the same fact
    /// is a reconciliation finding, not a second booking.
    pub fn event_id(&self) -> String {
        event_id_for(
            self.kind,
            self.module,
            self.venue,
            &self.wallet,
            &self.reference_id,
        )
    }

    /// Fee-inclusive cash out (buys) / gross proceeds (sells) — the figure
    /// the double-entry postings balance against.
    pub fn gross_quote(&self) -> f64 {
        match self.side {
            Some(EventSide::Sell) => self.quote_amount + self.fee,
            _ => self.quote_amount,
        }
    }

    /// Structural validation (no I/O). `Err` names the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.wallet.trim().is_empty() {
            return Err("wallet must not be empty".into());
        }
        if self.reference_id.trim().is_empty() {
            return Err("reference_id must not be empty".into());
        }
        if self.asset.trim().is_empty() {
            return Err("asset must not be empty".into());
        }
        if self.quote_asset.trim().is_empty() {
            return Err("quote_asset must not be empty".into());
        }
        for (label, v) in [
            ("quantity", self.quantity),
            ("quote_amount", self.quote_amount),
            ("fee", self.fee),
        ] {
            if !v.is_finite() {
                return Err(format!("{label} must be finite"));
            }
            if v < 0.0 {
                return Err(format!("{label} must be >= 0"));
            }
        }
        if let Some(p) = self.price {
            if !p.is_finite() || p < 0.0 {
                return Err("price must be finite and >= 0".into());
            }
        }
        match self.kind {
            EventKind::Fill | EventKind::Settlement => {
                if self.side.is_none() {
                    return Err(format!("{} requires a side", self.kind));
                }
                if self.quantity <= 0.0 {
                    return Err(format!("{} requires quantity > 0", self.kind));
                }
            }
            EventKind::Correction => {
                if self.side.is_none() {
                    return Err("correction requires a side".into());
                }
                if self.quantity <= 0.0 && self.quote_amount <= 0.0 {
                    return Err("correction must move quantity or cash".into());
                }
                if self
                    .correlation_id
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err("correction must reference the finding / ticket it answers".into());
                }
            }
            EventKind::Transfer => {
                if self
                    .counterparty_wallet
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err("transfer requires counterparty_wallet".into());
                }
                if self.quote_amount <= 0.0 {
                    return Err("transfer requires quote_amount > 0".into());
                }
            }
            EventKind::Deposit | EventKind::Withdrawal | EventKind::FundingAdjustment => {
                if self.quote_amount <= 0.0 && self.fee <= 0.0 {
                    return Err(format!("{} requires quote_amount > 0", self.kind));
                }
            }
            EventKind::Fee => {
                if self.fee <= 0.0 && self.quote_amount <= 0.0 {
                    return Err("fee event requires fee > 0".into());
                }
            }
        }
        Ok(())
    }

    /// Single-line `key=value` summary for audit records.
    pub fn summary(&self) -> String {
        format!(
            "kind={} module={} venue={} wallet={} strategy={} asset={} quote={} side={} qty={:.8} price={} amount={:.8} fee={:.8} mode={} ref={} corr={} position={} trade={}",
            self.kind,
            self.module,
            self.venue.as_str(),
            self.wallet,
            self.strategy,
            self.asset,
            self.quote_asset,
            self.side.map(|s| s.as_str()).unwrap_or("-"),
            self.quantity,
            self.price
                .map(|p| format!("{p:.8}"))
                .unwrap_or_else(|| "-".into()),
            self.quote_amount,
            self.fee,
            self.mode.as_str(),
            self.reference_id,
            self.correlation_id.as_deref().unwrap_or("-"),
            self.position_id.as_deref().unwrap_or("-"),
            self.trade_id.as_deref().unwrap_or("-"),
        )
    }

    /// Reference id for a module trade record: the transaction signature
    /// when the fill is on chain (a replayed confirmation of the same
    /// signature is then a duplicate), otherwise a paper reference that is
    /// unique across process lives (`paper:<trade id>@<ms>`).
    pub fn reference_for_trade(trade: &Trade) -> String {
        match trade.signature.as_deref().map(str::trim) {
            Some(sig) if !sig.is_empty() => sig.to_string(),
            _ => format!("paper:{}@{}", trade.id, trade.ts.timestamp_millis()),
        }
    }
}

/// The idempotency identity digest (see [`AccountingEvent::event_id`]).
pub fn event_id_for(
    kind: EventKind,
    module: BotModule,
    venue: Venue,
    wallet: &str,
    reference_id: &str,
) -> String {
    let mut h = Sha256::new();
    h.update(b"ledger-event-v1|");
    h.update(kind.as_str().as_bytes());
    h.update(b"|");
    h.update(module.as_str().as_bytes());
    h.update(b"|");
    h.update(venue.as_str().as_bytes());
    h.update(b"|");
    h.update(wallet.trim().as_bytes());
    h.update(b"|");
    h.update(reference_id.trim().as_bytes());
    format!("{EVENT_ID_PREFIX}{}", &hex::encode(h.finalize())[..40])
}

/// Builder for the most common event: a confirmed fill reported by a
/// trading module. Keeps the module call sites small and uniform.
#[allow(clippy::too_many_arguments)]
pub fn fill_event(
    module: BotModule,
    venue: Venue,
    wallet: impl Into<String>,
    strategy: impl Into<String>,
    asset: impl Into<String>,
    quote_asset: impl Into<String>,
    side: EventSide,
    quantity: f64,
    price: f64,
    quote_amount: f64,
    fee: f64,
    mode: ExecutionMode,
    reference_id: impl Into<String>,
    correlation_id: Option<String>,
    position_id: Option<String>,
    ts: DateTime<Utc>,
    detail: impl Into<String>,
) -> AccountingEvent {
    AccountingEvent {
        kind: EventKind::Fill,
        module,
        venue,
        wallet: wallet.into(),
        strategy: strategy.into(),
        asset: asset.into(),
        quote_asset: quote_asset.into(),
        side: Some(side),
        quantity,
        price: Some(price),
        quote_amount,
        fee,
        mode,
        reference_id: reference_id.into(),
        correlation_id,
        position_id,
        trade_id: None,
        counterparty_wallet: None,
        ts,
        detail: detail.into(),
    }
}

/// The fill event for one module [`Trade`] record: every field the trade
/// carries is copied verbatim (module, venue, mode, side, quantities,
/// price, fee, timestamp, position id, trade id) and the reference id is
/// [`AccountingEvent::reference_for_trade`] unless the caller supplies a
/// venue-level fill identity (`reference` — Polymarket passes its fill id).
pub fn fill_event_for_trade(
    trade: &Trade,
    wallet: impl Into<String>,
    strategy: impl Into<String>,
    reference: Option<String>,
    correlation_id: Option<String>,
) -> AccountingEvent {
    let buy = trade.is_buy();
    let (qty, quote) = if buy {
        (trade.amount_out, trade.amount_in)
    } else {
        (trade.amount_in, trade.amount_out)
    };
    let mut e = fill_event(
        match trade.source {
            crate::models::TradeSource::Copy => BotModule::Copy,
            crate::models::TradeSource::Polymarket => BotModule::Polymarket,
            crate::models::TradeSource::Sniper
            | crate::models::TradeSource::Manual
            | crate::models::TradeSource::Risk => BotModule::Sniper,
        },
        trade.venue,
        wallet,
        strategy,
        trade.symbol.clone(),
        trade.quote_symbol.clone(),
        if buy { EventSide::Buy } else { EventSide::Sell },
        qty,
        trade.price,
        quote,
        trade.fee,
        trade.mode,
        reference.unwrap_or_else(|| AccountingEvent::reference_for_trade(trade)),
        correlation_id,
        trade.position_id.clone(),
        trade.ts,
        trade.note.clone().unwrap_or_default(),
    );
    e.trade_id = Some(trade.id.clone());
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AccountingEvent {
        fill_event(
            BotModule::Sniper,
            Venue::PumpFun,
            "wallet-a",
            "sniper",
            "MINT",
            "SOL",
            EventSide::Buy,
            100.0,
            0.01,
            1.0,
            0.0,
            ExecutionMode::Paper,
            "sig-1",
            Some("order-1".into()),
            Some("p-1".into()),
            Utc::now(),
            "test",
        )
    }

    #[test]
    fn event_id_depends_only_on_identity_fields() {
        let a = base();
        let mut b = base();
        b.quantity = 5.0;
        b.quote_amount = 0.5;
        b.ts = Utc::now();
        b.detail = "different".into();
        assert_eq!(a.event_id(), b.event_id(), "amounts are not identity");
        let mut c = base();
        c.reference_id = "sig-2".into();
        assert_ne!(a.event_id(), c.event_id());
        let mut d = base();
        d.wallet = "wallet-b".into();
        assert_ne!(a.event_id(), d.event_id());
        let mut e = base();
        e.kind = EventKind::Settlement;
        assert_ne!(a.event_id(), e.event_id());
        assert!(a.event_id().starts_with(EVENT_ID_PREFIX));
        assert_eq!(a.event_id().len(), EVENT_ID_PREFIX.len() + 40);
    }

    #[test]
    fn validation_rejects_malformed_events() {
        assert!(base().validate().is_ok());
        let mut e = base();
        e.quantity = f64::NAN;
        assert!(e.validate().is_err());
        let mut e = base();
        e.quantity = -1.0;
        assert!(e.validate().is_err());
        let mut e = base();
        e.reference_id = " ".into();
        assert!(e.validate().is_err());
        let mut e = base();
        e.side = None;
        assert!(e.validate().is_err());
        let mut e = base();
        e.kind = EventKind::Transfer;
        e.side = None;
        assert!(e.validate().is_err(), "transfer without counterparty");
        e.counterparty_wallet = Some("wallet-b".into());
        assert!(e.validate().is_ok());
        let mut e = base();
        e.kind = EventKind::Correction;
        e.correlation_id = None;
        assert!(e.validate().is_err(), "correction must be referenced");
    }

    #[test]
    fn kind_labels_round_trip() {
        for k in EventKind::ALL {
            assert_eq!(EventKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EventKind::parse("nope"), None);
        assert_eq!(EventSide::parse("buy"), Some(EventSide::Buy));
        assert_eq!(EventSide::parse("sell"), Some(EventSide::Sell));
    }

    #[test]
    fn gross_quote_adds_the_fee_back_on_sells() {
        let mut e = base();
        e.side = Some(EventSide::Sell);
        e.quote_amount = 0.9;
        e.fee = 0.1;
        assert!((e.gross_quote() - 1.0).abs() < 1e-12);
        e.side = Some(EventSide::Buy);
        assert!((e.gross_quote() - 0.9).abs() < 1e-12);
    }
}
