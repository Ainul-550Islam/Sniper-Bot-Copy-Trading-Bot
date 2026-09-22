//! Order sizing, tick-size rounding and V2 order construction.
//!
//! Mirrors `py-clob-client`'s `OrderBuilder.get_order_amounts`: amounts are
//! rounded according to the market's tick size, then scaled to the 6-decimal
//! raw units both USDC and CTF outcome tokens use on Polygon. The result is a
//! [`OrderV2`] ready to be EIP-712 signed.
//!
//! ## Order pipeline vocabulary (TASK 4)
//! This module also owns the engine's order model: the canonical
//! [`OrderSignal`] (one strategy decision with a deterministic identity and
//! the ONE idempotency key handed to the OMS), the staged pipeline vocabulary
//! ([`PolyStage`], [`RejectReason`], [`SignalOutcome`]) and the venue-side
//! order lifecycle state machine ([`VenueOrderState`], [`LocalOrderState`],
//! [`TrackedOrder::apply_venue`]) that turns CLOB status polls and user-channel
//! events into fills and terminal transitions without ever double-booking.

use chrono::{DateTime, Utc};
use k256::ecdsa::SigningKey;

use bot_core::models::ExecutionMode;
use bot_core::oms::OrderStatus;
use bot_core::risk::RiskCode;

use crate::eip712::{self, OrderV2, SIDE_BUY, SIDE_SELL};
use crate::error::{PolyError, PolyResult};
use crate::strategy::OrderDecision;

/// USDC and CTF outcome tokens are both 6 decimals on Polygon.
pub const TOKEN_DECIMALS: u32 = 6;
/// 10^6 as a f64, for scaling human amounts to raw units.
const SCALE: f64 = 1_000_000.0;

/// Per-tick rounding precision (decimal places), matching the Python client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundConfig {
    /// Decimal places for the price.
    pub price: usize,
    /// Decimal places for the order size.
    pub size: usize,
    /// Decimal places for the computed raw amount.
    pub amount: usize,
}

impl RoundConfig {
    /// The rounding config for a tick size string ("0.1", "0.01", …).
    pub fn for_tick(tick: &str) -> PolyResult<Self> {
        Ok(match tick {
            "0.1" => RoundConfig {
                price: 1,
                size: 2,
                amount: 3,
            },
            "0.01" => RoundConfig {
                price: 2,
                size: 2,
                amount: 4,
            },
            "0.001" => RoundConfig {
                price: 3,
                size: 2,
                amount: 5,
            },
            "0.0001" => RoundConfig {
                price: 4,
                size: 2,
                amount: 6,
            },
            other => {
                return Err(PolyError::invalid(format!(
                    "unsupported tick size: {other}"
                )))
            }
        })
    }
}

/// Count the decimal places in a positive f64 (0 for integers).
fn decimal_places(x: f64) -> usize {
    if !x.is_finite() || x == 0.0 {
        return 0;
    }
    let s = format!("{x}");
    match s.split_once('.') {
        Some((_i, frac)) => frac.trim_end_matches('0').len(),
        None => 0,
    }
}

/// Round to `dp` decimal places, half away from zero.
fn round_normal(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).round() / factor
}

/// Truncate (round toward zero) at `dp` decimal places.
fn round_down(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).trunc() / factor
}

/// Round away from zero at `dp` decimal places.
fn round_up(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).ceil() / factor
}

/// Scale a human amount to 6-decimal raw units.
fn to_token_decimals(x: f64) -> u128 {
    let scaled = x * SCALE;
    // Guard against tiny negative drift from float arithmetic.
    if scaled <= 0.0 {
        0
    } else {
        scaled.round() as u128
    }
}

/// The computed raw amounts for an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderAmounts {
    /// `SIDE_BUY` or `SIDE_SELL`.
    pub side: u8,
    /// Amount the maker gives (USDC for a buy, tokens for a sell), raw 6dp.
    pub maker_amount: u128,
    /// Amount the maker receives (tokens for a buy, USDC for a sell), raw 6dp.
    pub taker_amount: u128,
}

/// Compute limit-order amounts for `size` outcome tokens at `price`.
///
/// `is_buy` selects the direction. Follows the Python client's rounding: the
/// size is truncated to the tick's size precision, the price is rounded to the
/// tick's price precision, and the product is nudged to the amount precision.
pub fn limit_order_amounts(is_buy: bool, size: f64, price: f64, cfg: RoundConfig) -> OrderAmounts {
    let raw_price = round_normal(price, cfg.price);
    if is_buy {
        let raw_taker = round_down(size, cfg.size);
        let mut raw_maker = raw_taker * raw_price;
        if decimal_places(raw_maker) > cfg.amount {
            raw_maker = round_up(raw_maker, cfg.amount + 4);
            if decimal_places(raw_maker) > cfg.amount {
                raw_maker = round_down(raw_maker, cfg.amount);
            }
        }
        OrderAmounts {
            side: SIDE_BUY,
            maker_amount: to_token_decimals(raw_maker),
            taker_amount: to_token_decimals(raw_taker),
        }
    } else {
        let raw_maker = round_down(size, cfg.size);
        let mut raw_taker = raw_maker * raw_price;
        if decimal_places(raw_taker) > cfg.amount {
            raw_taker = round_up(raw_taker, cfg.amount + 4);
            if decimal_places(raw_taker) > cfg.amount {
                raw_taker = round_down(raw_taker, cfg.amount);
            }
        }
        OrderAmounts {
            side: SIDE_SELL,
            maker_amount: to_token_decimals(raw_maker),
            taker_amount: to_token_decimals(raw_taker),
        }
    }
}

/// Parameters needed to assemble and sign an order.
pub struct OrderParams<'a> {
    /// The CTF ERC-1155 token id (decimal string).
    pub token_id: &'a str,
    /// Buy (true) or sell (false).
    pub is_buy: bool,
    /// Size in outcome tokens (human units).
    pub size: f64,
    /// Limit price in [0, 1] (human units).
    pub price: f64,
    /// Tick size string, e.g. "0.01".
    pub tick_size: &'a str,
    /// `true` for a neg-risk market (uses the neg-risk exchange as verifier).
    pub neg_risk: bool,
    /// Chain id (137 for Polygon).
    pub chain_id: u64,
    /// EIP-712 domain version ("2").
    pub domain_version: &'a str,
    /// The exchange contract that verifies the order.
    pub exchange_address: &'a str,
    /// The neg-risk exchange contract.
    pub neg_risk_exchange_address: &'a str,
    /// Signature type (0 EOA, 1 proxy, 2 safe, 3 deposit wallet).
    pub signature_type: u8,
    /// Address holding the funds (maker). Defaults to the signer when `None`.
    pub funder: Option<&'a str>,
    /// Order lifetime: `None`/0 for GTC, else a unix timestamp for GTD.
    pub expiration_timestamp: u64,
    /// Builder code (bytes32 hex) or `None` for zeros.
    pub builder_code: Option<&'a str>,
}

/// A built order plus the EIP-712 verifying contract used to sign it.
#[derive(Debug, Clone)]
pub struct SignedOrderBundle {
    /// The signed order fields.
    pub order: OrderV2,
    /// The `0x`-hex signature (r||s||v), possibly wrapped for type 3.
    pub signature: String,
    /// The exchange address that verifies this order.
    pub verifying_contract: String,
    /// Whether this is a neg-risk market.
    pub neg_risk: bool,
}

impl SignedOrderBundle {
    /// The CLOB order id this bundle will be assigned: the exchange's
    /// `getOrderHash(order)` equals `keccak256(abi.encode(Order))` — the
    /// EIP-712 struct hash — so it is derivable locally BEFORE (and without)
    /// the HTTP response. Reconciliation uses this to resolve submit-unknown
    /// outcomes (POST timed out: is the order resting/matched/absent?).
    pub fn derived_order_id(&self) -> PolyResult<String> {
        Ok(format!("0x{}", hex::encode(self.order.struct_hash()?)))
    }
}

/// Build and sign an order, returning the bundle the CLOB expects.
pub fn sign_order_bundle(key: &SigningKey, params: &OrderParams) -> PolyResult<SignedOrderBundle> {
    let signer_address = eip712::address_from_signing_key(key);
    let cfg = RoundConfig::for_tick(params.tick_size)?;
    let amounts = limit_order_amounts(params.is_buy, params.size, params.price, cfg);

    let verifying_contract = if params.neg_risk {
        params.neg_risk_exchange_address
    } else {
        params.exchange_address
    };

    let maker = match params.funder {
        Some(f) if !f.trim().is_empty() => f.to_string(),
        _ => signer_address.clone(),
    };

    // EOA signing (`signature_type` 0): the exchange recovers the signer from
    // the signature and requires it to BE the maker. A funder that is not the
    // signer is only valid with a proxy / safe / deposit-wallet type (1–3).
    // Refuse locally — a typed `SIGNING_FAILED` rejection before any live
    // POST — instead of letting the venue bounce the order.
    if params.signature_type == 0 && !maker.eq_ignore_ascii_case(&signer_address) {
        return Err(PolyError::invalid(format!(
            "signature_type 0 (EOA) requires maker == signer: funder_address {maker} is not the \
             signing key's address {signer_address} (use signature_type 1-3 for a proxy/safe funder)"
        )));
    }

    // Deterministic salt (Prompt 2 §G/§Q): the CLOB order id is
    // keccak256(abi.encode(Order)), so deriving the salt purely from the
    // intent's semantic content makes a re-signed copy of the SAME intent —
    // after an ambiguous HTTP timeout, a crash/restart, or a strategy retry —
    // map to the SAME order id. The venue then deduplicates it instead of
    // resting a second live order (never double-trade the same intent).
    // GTD intents carry their expiration in `timestamp`, so each new expiry
    // window naturally produces a fresh order id; deliberately re-ordering an
    // identical GTC intent requires changing size/price.
    let mut salt_pre: Vec<u8> = Vec::with_capacity(256);
    salt_pre.extend_from_slice(b"polymarket-salt-v1|");
    salt_pre.extend_from_slice(maker.as_bytes());
    salt_pre.push(b'|');
    salt_pre.extend_from_slice(signer_address.as_bytes());
    salt_pre.extend_from_slice(
        format!(
            "|{}|{}|{}|{}|{}|{}|{}",
            params.token_id,
            amounts.maker_amount,
            amounts.taker_amount,
            amounts.side,
            params.signature_type,
            params.expiration_timestamp,
            params.builder_code.unwrap_or("")
        )
        .as_bytes(),
    );
    let salt_hash = eip712::keccak256(&salt_pre);
    let salt: u128 = u128::from_be_bytes(
        salt_hash[..16]
            .try_into()
            .expect("keccak output is 32 bytes"),
    ) | 1; // never zero
    let order = OrderV2 {
        salt: salt.to_string(),
        maker,
        signer: signer_address.clone(),
        token_id: params.token_id.to_string(),
        maker_amount: amounts.maker_amount.to_string(),
        taker_amount: amounts.taker_amount.to_string(),
        side: amounts.side,
        signature_type: params.signature_type,
        timestamp: params.expiration_timestamp.to_string(),
        metadata: String::new(),
        builder: params.builder_code.unwrap_or("").to_string(),
    };

    let digest = eip712::order_digest(
        params.chain_id,
        params.domain_version,
        verifying_contract,
        &order,
    )?;
    let raw_sig = eip712::sign_digest(key, &digest)?;

    // Type 3 (deposit wallet / EIP-1271) wraps the inner signature.
    let signature = if params.signature_type == 3 {
        let wrapped = eip712::wrap_deposit_wallet_signature(
            &raw_sig,
            params.chain_id,
            params.domain_version,
            verifying_contract,
            &order,
        )?;
        format!("0x{}", hex::encode(wrapped))
    } else {
        format!("0x{}", hex::encode(raw_sig))
    };

    Ok(SignedOrderBundle {
        order,
        signature,
        verifying_contract: verifying_contract.to_string(),
        neg_risk: params.neg_risk,
    })
}

// ---------------------------------------------------------------------------
// Order pipeline vocabulary (TASK 4)
// ---------------------------------------------------------------------------

/// Round a limit price to the tick grid (`tick` = `"0.01"`, `"0.001"`, …).
/// Used for the intent identity so two decisions that the CLOB would treat
/// as the same price share one idempotency key.
pub fn round_price_to_tick(price: f64, tick: &str) -> f64 {
    let decimals = RoundConfig::for_tick(tick).map(|c| c.price).unwrap_or(2);
    let scale = 10f64.powi(decimals as i32);
    (price * scale).round() / scale
}

/// The canonical order signal: one strategy decision, frozen with everything
/// that defines its identity. Built once per decision (never mutated after
/// construction) so every stage of the pipeline reasons about the same
/// object and `signal_id` / `intent_key` stay stable across retries,
/// crashes and replicas.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderSignal {
    /// Deterministic signal id: `psig_` + 32 hex of the intent identity.
    pub signal_id: String,
    /// The strategy decision as produced (never re-priced in place).
    pub decision: OrderDecision,
    /// Canonical strategy label (`value` | `search`).
    pub strategy: &'static str,
    /// Execution mode the decision was made in.
    pub mode: ExecutionMode,
    /// Order type on the venue (`GTC` | `GTD` | `FOK` | `FAK`).
    pub order_type: String,
    /// Tick size of the market (price grid).
    pub tick_size: String,
    /// Unix seconds; 0 = no expiry (GTC).
    pub expiration: u64,
    /// Market question (display only).
    pub question: String,
    /// When the signal was created.
    pub created_at: DateTime<Utc>,
}

impl OrderSignal {
    /// Freeze a decision into a signal. `expiration` is the GTD deadline the
    /// caller computed (0 for GTC).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        decision: OrderDecision,
        strategy: &'static str,
        mode: ExecutionMode,
        order_type: &str,
        tick_size: &str,
        expiration: u64,
        question: &str,
        created_at: DateTime<Utc>,
    ) -> Self {
        let order_type = order_type.trim().to_ascii_uppercase();
        let key = intent_key(&decision, mode, &order_type, tick_size, expiration);
        let signal_id = format!("psig_{}", &key[..32.min(key.len())]);
        OrderSignal {
            signal_id,
            decision,
            strategy,
            mode,
            order_type,
            tick_size: tick_size.to_string(),
            expiration,
            question: question.to_string(),
            created_at,
        }
    }

    /// The ONE idempotency key for this signal — the OMS `idempotency_key`.
    /// Identical for a re-evaluated / retried / replica-duplicated copy of
    /// the same intent; different whenever price (on the tick grid), size,
    /// side, order type, expiry window or mode differ.
    pub fn intent_key(&self) -> String {
        intent_key(
            &self.decision,
            self.mode,
            &self.order_type,
            &self.tick_size,
            self.expiration,
        )
    }

    /// `buy` | `sell`.
    pub fn side_str(&self) -> &'static str {
        if self.decision.is_buy {
            "buy"
        } else {
            "sell"
        }
    }

    /// Outcome token id.
    pub fn token_id(&self) -> &str {
        &self.decision.token_id
    }

    /// Market condition id.
    pub fn condition_id(&self) -> &str {
        &self.decision.condition_id
    }

    /// Deterministic PAPER venue-order id (paper fills never touch the CLOB
    /// but still need a stable handle in the journal).
    pub fn paper_order_id(&self) -> String {
        format!("paper:{}", &self.intent_key()[..40])
    }
}

/// The deterministic intent identity shared by the OMS idempotency key and
/// the signal id: sha256 over the LOGICAL order (market, token, side, tick-
/// rounded price, 2-dp size, order type, expiry, mode). Never includes
/// timestamps, attempt counters or venue ids.
pub fn intent_key(
    decision: &OrderDecision,
    mode: ExecutionMode,
    order_type: &str,
    tick_size: &str,
    expiration: u64,
) -> String {
    let price = round_price_to_tick(decision.limit_price, tick_size);
    let size = (decision.size_tokens * 100.0).floor() / 100.0;
    bot_core::auth::sha256_hex(&format!(
        "poly-intent-v1|{}|{}|{}|{:.4}|{:.2}|{}|{}|{}",
        decision.condition_id.trim().to_ascii_lowercase(),
        decision.token_id.trim(),
        if decision.is_buy { "buy" } else { "sell" },
        price,
        size,
        order_type.trim().to_ascii_uppercase(),
        expiration,
        mode.as_str()
    ))
}

/// Pipeline stages, in order. Every signal ends in exactly one terminal
/// stage (`Filled`, `Resting`, `Ambiguous`, `Rejected` or `Failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolyStage {
    /// The decision entered the pipeline.
    Received,
    /// Shape checks passed (ids, price range, finite positive size).
    Validated,
    /// The market is tradeable (active, accepting, resolution window).
    MarketResolved,
    /// A fresh, two-sided, tight enough quote exists and the limit price is
    /// inside the tolerated band around it.
    Quoted,
    /// No open position / resting order for the token, reconciliation gate
    /// clear, open-market cap respected.
    ExposureChecked,
    /// Size rounded to the venue grid and above the minimum.
    Sized,
    /// The shared risk engine allowed (and possibly resized) the entry.
    RiskApproved,
    /// LIVE only: on-chain collateral balance + allowance verified.
    CollateralVerified,
    /// The OMS accepted the intent (idempotency gate passed).
    Idempotent,
    /// Distributed ownership of `poly:entry:{token}` claimed.
    OwnershipClaimed,
    /// The EIP-712 order is signed (venue order id derivable).
    Signed,
    /// The order reached the venue (or the paper book).
    Submitted,
    /// Terminal: fully filled (immediately or paper).
    Filled,
    /// Terminal for the pipeline: the order rests on the book; the lifecycle
    /// tracker owns it from here.
    Resting,
    /// Terminal for the pipeline: the POST outcome is unknown; reconciliation
    /// owns the order.
    Ambiguous,
    /// Terminal: refused before anything left the process.
    Rejected,
    /// Terminal: an error after the ownership claim (definite, not resting).
    Failed,
}

impl PolyStage {
    /// Stable SCREAMING_SNAKE label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PolyStage::Received => "RECEIVED",
            PolyStage::Validated => "VALIDATED",
            PolyStage::MarketResolved => "MARKET_RESOLVED",
            PolyStage::Quoted => "QUOTED",
            PolyStage::ExposureChecked => "EXPOSURE_CHECKED",
            PolyStage::Sized => "SIZED",
            PolyStage::RiskApproved => "RISK_APPROVED",
            PolyStage::CollateralVerified => "COLLATERAL_VERIFIED",
            PolyStage::Idempotent => "IDEMPOTENT",
            PolyStage::OwnershipClaimed => "OWNERSHIP_CLAIMED",
            PolyStage::Signed => "SIGNED",
            PolyStage::Submitted => "SUBMITTED",
            PolyStage::Filled => "FILLED",
            PolyStage::Resting => "RESTING",
            PolyStage::Ambiguous => "AMBIGUOUS",
            PolyStage::Rejected => "REJECTED",
            PolyStage::Failed => "FAILED",
        }
    }

    /// Whether the pipeline stops at this stage.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            PolyStage::Filled
                | PolyStage::Resting
                | PolyStage::Ambiguous
                | PolyStage::Rejected
                | PolyStage::Failed
        )
    }

    /// Every stage, in pipeline order.
    pub const ALL: [PolyStage; 17] = [
        PolyStage::Received,
        PolyStage::Validated,
        PolyStage::MarketResolved,
        PolyStage::Quoted,
        PolyStage::ExposureChecked,
        PolyStage::Sized,
        PolyStage::RiskApproved,
        PolyStage::CollateralVerified,
        PolyStage::Idempotent,
        PolyStage::OwnershipClaimed,
        PolyStage::Signed,
        PolyStage::Submitted,
        PolyStage::Filled,
        PolyStage::Resting,
        PolyStage::Ambiguous,
        PolyStage::Rejected,
        PolyStage::Failed,
    ];
}

impl std::fmt::Display for PolyStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a signal was rejected or failed. Stable labels for journal, metrics
/// and audit; `from_risk_code` maps the shared risk engine's codes 1:1 so
/// there is never a second vocabulary for the same refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// Token / condition id malformed.
    InvalidToken,
    /// Limit price outside (0, 1).
    InvalidPrice,
    /// Size not a positive finite number.
    InvalidSize,
    /// Market gate failed (inactive / closed / not accepting).
    MarketNotTradeable,
    /// Market resolves inside the protection window.
    ResolvingSoon,
    /// Market liquidity below the floor.
    LowLiquidity,
    /// No quote for the token.
    NoQuote,
    /// Quote older than `quote_max_age_secs`.
    StaleQuote,
    /// Spread wider than `max_spread`.
    SpreadTooWide,
    /// Book one-sided or crossed.
    BadBook,
    /// Limit price outside the tolerated band around the current book.
    PriceOutsideBand,
    /// Already holding this outcome token.
    AlreadyInMarket,
    /// A non-terminal order for this token already exists.
    OrderAlreadyOpen,
    /// `max_open_markets` reached.
    MaxOpenMarkets,
    /// The token is gated by unresolved reconciliation.
    SymbolGated,
    /// Rounded size below `min_order_size`.
    SizeTooSmall,
    /// Shared risk engine: kill switch.
    KillSwitch,
    /// Shared risk engine: module disabled.
    ModuleDisabled,
    /// Shared risk engine: daily loss (generic or Polymarket-scoped).
    DailyLossLimit,
    /// Shared risk engine: `poly_emergency_disable`.
    EmergencyDisabled,
    /// Shared risk engine: too many open positions.
    MaxOpenPositions,
    /// Shared risk engine: resting-order cap.
    OpenOrderCap,
    /// Shared risk engine: total / per-market exposure cap.
    ExposureCap,
    /// Shared risk engine: balance below reserve.
    InsufficientBalance,
    /// Shared risk engine: duplicate symbol / re-entry cooldown / other.
    RiskRejected,
    /// LIVE: collateral balance / decimals unreadable.
    CollateralUnavailable,
    /// LIVE: balance or allowance below the approved notional.
    InsufficientFunding,
    /// The OMS already holds this intent (idempotency gate).
    DuplicateIntent,
    /// The OMS refused the draft.
    OmsRejected,
    /// Another replica owns `poly:entry:{token}`.
    OwnedByOtherReplica,
    /// Ownership store unavailable / fencing refused (fail closed).
    OwnershipUnavailable,
    /// No signer / API key for a live order.
    NotConfigured,
    /// EIP-712 signing failed.
    SigningFailed,
    /// The venue rejected the order (definite).
    VenueRejected,
    /// Transport failure after signing (order may rest) — see `Ambiguous`.
    SubmitUnknown,
    /// Any other internal error.
    Internal,
}

impl RejectReason {
    /// Stable SCREAMING_SNAKE label.
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectReason::InvalidToken => "INVALID_TOKEN",
            RejectReason::InvalidPrice => "INVALID_PRICE",
            RejectReason::InvalidSize => "INVALID_SIZE",
            RejectReason::MarketNotTradeable => "MARKET_NOT_TRADEABLE",
            RejectReason::ResolvingSoon => "RESOLVING_SOON",
            RejectReason::LowLiquidity => "LOW_LIQUIDITY",
            RejectReason::NoQuote => "NO_QUOTE",
            RejectReason::StaleQuote => "STALE_QUOTE",
            RejectReason::SpreadTooWide => "SPREAD_TOO_WIDE",
            RejectReason::BadBook => "BAD_BOOK",
            RejectReason::PriceOutsideBand => "PRICE_OUTSIDE_BAND",
            RejectReason::AlreadyInMarket => "ALREADY_IN_MARKET",
            RejectReason::OrderAlreadyOpen => "ORDER_ALREADY_OPEN",
            RejectReason::MaxOpenMarkets => "MAX_OPEN_MARKETS",
            RejectReason::SymbolGated => "SYMBOL_GATED",
            RejectReason::SizeTooSmall => "SIZE_TOO_SMALL",
            RejectReason::KillSwitch => "KILL_SWITCH",
            RejectReason::ModuleDisabled => "MODULE_DISABLED",
            RejectReason::DailyLossLimit => "DAILY_LOSS_LIMIT",
            RejectReason::EmergencyDisabled => "EMERGENCY_DISABLED",
            RejectReason::MaxOpenPositions => "MAX_OPEN_POSITIONS",
            RejectReason::OpenOrderCap => "OPEN_ORDER_CAP",
            RejectReason::ExposureCap => "EXPOSURE_CAP",
            RejectReason::InsufficientBalance => "INSUFFICIENT_BALANCE",
            RejectReason::RiskRejected => "RISK_REJECTED",
            RejectReason::CollateralUnavailable => "COLLATERAL_UNAVAILABLE",
            RejectReason::InsufficientFunding => "INSUFFICIENT_FUNDING",
            RejectReason::DuplicateIntent => "DUPLICATE_INTENT",
            RejectReason::OmsRejected => "OMS_REJECTED",
            RejectReason::OwnedByOtherReplica => "OWNED_BY_OTHER_REPLICA",
            RejectReason::OwnershipUnavailable => "OWNERSHIP_UNAVAILABLE",
            RejectReason::NotConfigured => "NOT_CONFIGURED",
            RejectReason::SigningFailed => "SIGNING_FAILED",
            RejectReason::VenueRejected => "VENUE_REJECTED",
            RejectReason::SubmitUnknown => "SUBMIT_UNKNOWN",
            RejectReason::Internal => "INTERNAL",
        }
    }

    /// Map the shared risk engine's code onto the pipeline vocabulary.
    pub fn from_risk_code(code: Option<RiskCode>) -> Self {
        match code {
            Some(RiskCode::KillSwitch) | Some(RiskCode::GlobalKillSwitch) => {
                RejectReason::KillSwitch
            }
            Some(RiskCode::ModuleDisabled) => RejectReason::ModuleDisabled,
            Some(RiskCode::DailyLossLimit)
            | Some(RiskCode::PolyDailyLoss)
            | Some(RiskCode::GlobalDailyLoss) => RejectReason::DailyLossLimit,
            Some(RiskCode::PolyEmergencyDisabled) => RejectReason::EmergencyDisabled,
            Some(RiskCode::InvalidSize) => RejectReason::InvalidSize,
            Some(RiskCode::MaxOpenPositions) => RejectReason::MaxOpenPositions,
            Some(RiskCode::DuplicateSymbol) => RejectReason::AlreadyInMarket,
            Some(RiskCode::PolyOpenOrderCap) => RejectReason::OpenOrderCap,
            Some(RiskCode::ExposureCap)
            | Some(RiskCode::PolyMarketExposure)
            | Some(RiskCode::GlobalExposure)
            | Some(RiskCode::GlobalDrawdown) => RejectReason::ExposureCap,
            Some(RiskCode::InsufficientBalance) => RejectReason::InsufficientBalance,
            _ => RejectReason::RiskRejected,
        }
    }

    /// Map a strategy skip onto the pipeline vocabulary.
    pub fn from_skip(reason: crate::strategy::SkipReason) -> Self {
        use crate::strategy::SkipReason as S;
        match reason {
            S::MarketInactive | S::MarketClosed | S::NotAcceptingOrders => {
                RejectReason::MarketNotTradeable
            }
            S::ResolvingSoon => RejectReason::ResolvingSoon,
            S::LowLiquidity => RejectReason::LowLiquidity,
            S::NoQuote | S::NotBinary => RejectReason::NoQuote,
            S::OneSidedBook | S::CrossedBook => RejectReason::BadBook,
            S::SpreadTooWide => RejectReason::SpreadTooWide,
            S::StaleQuote => RejectReason::StaleQuote,
            S::PriceOutOfRange => RejectReason::InvalidPrice,
            S::SizeTooSmall => RejectReason::SizeTooSmall,
            S::InvalidStake => RejectReason::InvalidSize,
            S::NoEdge | S::NoKeywordMatch | S::NoKeywordsConfigured | S::UnknownStrategy => {
                RejectReason::RiskRejected
            }
        }
    }

    /// Map a module error onto the pipeline vocabulary.
    pub fn from_error(e: &PolyError) -> Self {
        match e {
            PolyError::SubmitUnknown { .. } | PolyError::Http(_) => RejectReason::SubmitUnknown,
            PolyError::Clob(_) => RejectReason::VenueRejected,
            PolyError::Signing(_) | PolyError::Encoding(_) => RejectReason::SigningFailed,
            PolyError::NotConfigured(_) => RejectReason::NotConfigured,
            PolyError::BalanceUnavailable(_) => RejectReason::CollateralUnavailable,
            PolyError::InsufficientFunding(_) => RejectReason::InsufficientFunding,
            PolyError::Invalid(_)
            | PolyError::Ws(_)
            | PolyError::Lifecycle(_)
            | PolyError::Journal(_) => RejectReason::Internal,
        }
    }

    /// Every variant, for exhaustive metering / docs.
    pub const ALL: [RejectReason; 36] = [
        RejectReason::InvalidToken,
        RejectReason::InvalidPrice,
        RejectReason::InvalidSize,
        RejectReason::MarketNotTradeable,
        RejectReason::ResolvingSoon,
        RejectReason::LowLiquidity,
        RejectReason::NoQuote,
        RejectReason::StaleQuote,
        RejectReason::SpreadTooWide,
        RejectReason::BadBook,
        RejectReason::PriceOutsideBand,
        RejectReason::AlreadyInMarket,
        RejectReason::OrderAlreadyOpen,
        RejectReason::MaxOpenMarkets,
        RejectReason::SymbolGated,
        RejectReason::SizeTooSmall,
        RejectReason::KillSwitch,
        RejectReason::ModuleDisabled,
        RejectReason::DailyLossLimit,
        RejectReason::EmergencyDisabled,
        RejectReason::MaxOpenPositions,
        RejectReason::OpenOrderCap,
        RejectReason::ExposureCap,
        RejectReason::InsufficientBalance,
        RejectReason::RiskRejected,
        RejectReason::CollateralUnavailable,
        RejectReason::InsufficientFunding,
        RejectReason::DuplicateIntent,
        RejectReason::OmsRejected,
        RejectReason::OwnedByOtherReplica,
        RejectReason::OwnershipUnavailable,
        RejectReason::NotConfigured,
        RejectReason::SigningFailed,
        RejectReason::VenueRejected,
        RejectReason::SubmitUnknown,
        RejectReason::Internal,
    ];
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where one signal ended and what it produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalOutcome {
    /// The signal id.
    pub signal_id: String,
    /// Terminal stage reached.
    pub stage: PolyStage,
    /// Set for `Rejected` / `Failed` / `Ambiguous`.
    pub reject_reason: Option<RejectReason>,
    /// Human-readable detail.
    pub detail: String,
    /// OMS order id once the idempotency gate was passed.
    pub order_id: Option<String>,
    /// Venue order id once signed (or the paper id).
    pub venue_order_id: Option<String>,
    /// Position id when a fill was booked.
    pub position_id: Option<String>,
    /// Risk-approved notional (USDC).
    pub approved_stake: Option<f64>,
    /// Final size in outcome tokens.
    pub size_tokens: Option<f64>,
    /// Wall time spent in the pipeline.
    pub total_ms: u64,
}

impl SignalOutcome {
    /// Whether the signal produced (or may have produced) a venue order.
    pub fn reached_venue(&self) -> bool {
        matches!(
            self.stage,
            PolyStage::Filled | PolyStage::Resting | PolyStage::Ambiguous
        )
    }
}

// ---------------------------------------------------------------------------
// Venue order lifecycle
// ---------------------------------------------------------------------------

/// Normalised CLOB order status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VenueOrderState {
    /// Resting on the book (`live`).
    Live,
    /// Fully matched (`matched` / `filled`).
    Matched,
    /// Marketable but held for the delay window (`delayed`).
    Delayed,
    /// Placed but not (yet) matched (`unmatched`) — treated as resting.
    Unmatched,
    /// Cancelled (`cancelled` / `canceled`).
    Cancelled,
    /// Expired (`expired`).
    Expired,
    /// Anything the venue says that we do not recognise.
    Unknown,
}

impl VenueOrderState {
    /// Parse the venue's free-form status string (case-insensitive).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "live" | "open" => VenueOrderState::Live,
            "matched" | "filled" => VenueOrderState::Matched,
            "delayed" => VenueOrderState::Delayed,
            "unmatched" | "placement" => VenueOrderState::Unmatched,
            "cancelled" | "canceled" => VenueOrderState::Cancelled,
            "expired" => VenueOrderState::Expired,
            _ => VenueOrderState::Unknown,
        }
    }

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            VenueOrderState::Live => "live",
            VenueOrderState::Matched => "matched",
            VenueOrderState::Delayed => "delayed",
            VenueOrderState::Unmatched => "unmatched",
            VenueOrderState::Cancelled => "cancelled",
            VenueOrderState::Expired => "expired",
            VenueOrderState::Unknown => "unknown",
        }
    }

    /// Whether the venue considers the order finished.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            VenueOrderState::Matched | VenueOrderState::Cancelled | VenueOrderState::Expired
        )
    }
}

/// Local lifecycle state of one venue order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalOrderState {
    /// POSTed; no venue confirmation yet.
    Submitted,
    /// Confirmed resting, nothing matched.
    Resting,
    /// Some size matched, some still resting.
    PartiallyFilled,
    /// The venue reported the order matched: everything for `GTC`/`GTD`/`FOK`;
    /// for `FAK` exactly the quantity the venue reported (the killed
    /// remainder is never booked, `remaining()` may be > 0).
    Filled,
    /// Cancelled by us or the venue (a partial fill stays booked).
    Cancelled,
    /// GTD expiry reached on the venue.
    Expired,
    /// The venue does not know the order and we never saw it accepted.
    Unknown,
    /// Definite failure (venue rejected after acceptance, lifecycle error).
    Failed,
}

impl LocalOrderState {
    /// Stable snake_case label (journal column).
    pub fn as_str(&self) -> &'static str {
        match self {
            LocalOrderState::Submitted => "submitted",
            LocalOrderState::Resting => "resting",
            LocalOrderState::PartiallyFilled => "partially_filled",
            LocalOrderState::Filled => "filled",
            LocalOrderState::Cancelled => "cancelled",
            LocalOrderState::Expired => "expired",
            LocalOrderState::Unknown => "unknown",
            LocalOrderState::Failed => "failed",
        }
    }

    /// Parse the journal label back (restart recovery).
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "submitted" => LocalOrderState::Submitted,
            "resting" => LocalOrderState::Resting,
            "partially_filled" => LocalOrderState::PartiallyFilled,
            "filled" => LocalOrderState::Filled,
            "cancelled" => LocalOrderState::Cancelled,
            "expired" => LocalOrderState::Expired,
            "failed" => LocalOrderState::Failed,
            _ => LocalOrderState::Unknown,
        }
    }

    /// Whether the order is finished locally.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            LocalOrderState::Filled
                | LocalOrderState::Cancelled
                | LocalOrderState::Expired
                | LocalOrderState::Failed
        )
    }

    /// The OMS status this local state corresponds to.
    pub fn oms_status(&self) -> OrderStatus {
        match self {
            LocalOrderState::Submitted => OrderStatus::Submitted,
            LocalOrderState::Resting => OrderStatus::Accepted,
            LocalOrderState::PartiallyFilled => OrderStatus::PartiallyFilled,
            LocalOrderState::Filled => OrderStatus::Filled,
            LocalOrderState::Cancelled => OrderStatus::Cancelled,
            LocalOrderState::Expired => OrderStatus::Expired,
            LocalOrderState::Unknown => OrderStatus::Unknown,
            LocalOrderState::Failed => OrderStatus::Failed,
        }
    }

    /// Legal local transitions. Terminal states never move; `Unknown` may
    /// resolve to anything once the venue answers.
    pub fn can_transition_to(&self, to: LocalOrderState) -> bool {
        use LocalOrderState as L;
        if *self == to {
            return false;
        }
        match self {
            L::Submitted => true,
            L::Resting => !matches!(to, L::Submitted),
            L::PartiallyFilled => matches!(
                to,
                L::Filled | L::Cancelled | L::Expired | L::Unknown | L::Failed
            ),
            L::Unknown => true,
            L::Filled | L::Cancelled | L::Expired | L::Failed => false,
        }
    }
}

/// Where a lifecycle observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillSource {
    /// `GET /data/order` poll.
    Poll,
    /// Authenticated user websocket channel.
    UserWs,
    /// Paper fill (no venue).
    Paper,
    /// Reconciliation against `/data/orders` or `/data/trades`.
    Recon,
}

impl FillSource {
    /// Stable label (journal column / metric label).
    pub fn as_str(&self) -> &'static str {
        match self {
            FillSource::Poll => "poll",
            FillSource::UserWs => "user_ws",
            FillSource::Paper => "paper",
            FillSource::Recon => "recon",
        }
    }
}

/// One venue order the engine is tracking until it is terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedOrder {
    /// Venue order id (`0x…` struct hash) or `paper:…`.
    pub venue_order_id: String,
    /// OMS order id.
    pub order_id: String,
    /// Signal id that produced it.
    pub signal_id: String,
    /// Market condition id.
    pub condition_id: String,
    /// Outcome token id.
    pub token_id: String,
    /// Outcome label.
    pub outcome: String,
    /// Market question (display).
    pub question: String,
    /// Buy / sell.
    pub is_buy: bool,
    /// Whether the market is neg-risk.
    pub neg_risk: bool,
    /// `GTC` | `GTD` | `FOK` | `FAK`.
    pub order_type: String,
    /// Limit price.
    pub limit_price: f64,
    /// Original size in outcome tokens.
    pub size_tokens: f64,
    /// Cumulative matched size booked so far.
    pub size_matched: f64,
    /// Execution mode.
    pub mode: ExecutionMode,
    /// Local state.
    pub state: LocalOrderState,
    /// Last raw venue status.
    pub venue_status: String,
    /// Unix seconds; 0 = GTC.
    pub expiration: u64,
    /// Position the fills are booked into (once any fill happened).
    pub position_id: Option<String>,
    /// EIP-712 signature (live) for OMS attachment.
    pub signature: Option<String>,
    /// When the order was submitted.
    pub submitted_at: DateTime<Utc>,
    /// Last observation.
    pub updated_at: DateTime<Utc>,
    /// Venue trade ids already booked (user-channel replay protection).
    pub booked_trade_ids: Vec<String>,
    /// Sum of the distinct per-trade deltas (user-channel `trade` events)
    /// seen in this process. A per-trade delta is booked only where this
    /// sum exceeds the cumulative the venue has already reported, so a trade
    /// whose quantity a poll captured first (its event arrived late, after a
    /// reconnect) is never booked twice. Not journaled; re-based to the
    /// reported cumulative by observations that name their trades
    /// (`associate_trades`).
    pub trade_matched: f64,
}

/// An observation of the order's venue state.
#[derive(Debug, Clone, PartialEq)]
pub struct VenueObservation {
    /// Normalised venue state.
    pub state: VenueOrderState,
    /// Raw status string.
    pub raw_status: String,
    /// Cumulative matched size reported by the venue (`None` = not reported
    /// by this event, e.g. a status-only update).
    pub size_matched: Option<f64>,
    /// Incremental fill size from ONE trade event (user channel `trade`,
    /// `/data/trades` row). Mutually exclusive with `size_matched`.
    pub fill_delta: Option<f64>,
    /// Venue trade id when the observation is one trade event.
    pub trade_id: Option<String>,
    /// Fill price for a trade event (`None` = use the limit price).
    pub price: Option<f64>,
    /// Where it came from.
    pub source: FillSource,
    /// When it was observed.
    pub at: DateTime<Utc>,
    /// Venue trade ids that make up `size_matched` (`associate_trades` on
    /// `GET /data/order`, the open-order list and user-channel `order`
    /// events; empty when not reported). They are remembered so later
    /// per-trade events for them are no-ops, and a cumulative that names its
    /// trades re-bases `trade_matched`.
    pub associate_trades: Vec<String>,
}

/// What applying an observation produced.
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleEffect {
    /// New size to book as a fill (`0` = nothing new).
    pub fill_delta: f64,
    /// Price to book the delta at.
    pub fill_price: f64,
    /// Local state before.
    pub from: LocalOrderState,
    /// Local state after.
    pub to: LocalOrderState,
    /// Whether the local state changed.
    pub transitioned: bool,
    /// The venue trade id booked (if any).
    pub trade_id: Option<String>,
}

impl TrackedOrder {
    /// Size still resting (never negative).
    pub fn remaining(&self) -> f64 {
        (self.size_tokens - self.size_matched).max(0.0)
    }

    /// USDC still committed by the unfilled part of a BUY (0 for sells).
    pub fn resting_quote(&self) -> f64 {
        if self.is_buy && !self.state.is_terminal() {
            self.remaining() * self.limit_price
        } else {
            0.0
        }
    }

    /// Age in seconds at `now`.
    pub fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.submitted_at)
            .num_seconds()
            .max(0)
    }

    /// Whether the local TTL (`order_ttl_secs`, 0 = off) has elapsed.
    pub fn ttl_elapsed(&self, now: DateTime<Utc>, ttl_secs: i64) -> bool {
        ttl_secs > 0 && !self.state.is_terminal() && self.age_secs(now) >= ttl_secs
    }

    /// Whether the GTD expiry has passed on the wall clock (grace of 1 s).
    pub fn expiry_passed(&self, now: DateTime<Utc>) -> bool {
        self.expiration > 0 && (now.timestamp().max(0) as u64) > self.expiration
    }

    /// Whether a resting BUY sits more than `threshold` below the best ask
    /// (`0` = never): a candidate for cancel + re-quote.
    pub fn needs_reprice(&self, best_ask: f64, threshold: f64) -> bool {
        threshold > 0.0
            && self.is_buy
            && !self.state.is_terminal()
            && best_ask > 0.0
            && best_ask - self.limit_price > threshold + 1e-12
    }

    /// Fill-and-kill (`FAK`) orders report `matched` for ANY non-zero fill
    /// and kill the remainder; every other type (`GTC`/`GTD`/`FOK`) reports
    /// `matched` only once the whole order matched.
    pub fn is_fill_and_kill(&self) -> bool {
        self.order_type.eq_ignore_ascii_case("FAK")
    }

    /// Whether the venue has acknowledged this order with a recognised
    /// status (the `POST /order` answer or a later observation). Orders
    /// whose status is still a local marker (empty, `submit_unknown`,
    /// `vanished from venue`, …) were never confirmed by the venue, so a
    /// `404` for them is a definite "never existed" rather than a lost fill.
    pub fn venue_acknowledged(&self) -> bool {
        VenueOrderState::parse(&self.venue_status) != VenueOrderState::Unknown
    }

    /// Remember a venue trade id without booking anything — used when the
    /// durable fill journal proves the trade was already booked before a
    /// restart, so the replay stays a no-op for the rest of this process.
    pub fn remember_trade(&mut self, trade_id: &str) {
        if !trade_id.trim().is_empty() && !self.booked_trade_ids.iter().any(|t| t == trade_id) {
            self.booked_trade_ids.push(trade_id.to_string());
        }
    }

    /// Apply one venue observation. Pure: returns the effect (fill delta +
    /// transition) and mutates only this struct. Never books the same
    /// quantity twice: the fill delta is `max(0, venue_matched - booked)`,
    /// a trade id already seen yields no delta, and a matched size above the
    /// order size is a lifecycle error, not a fill.
    pub fn apply_venue(&mut self, obs: &VenueObservation) -> PolyResult<LifecycleEffect> {
        let from = self.state;
        let mut effect = LifecycleEffect {
            fill_delta: 0.0,
            fill_price: obs.price.filter(|p| *p > 0.0).unwrap_or(self.limit_price),
            from,
            to: from,
            transitioned: false,
            trade_id: None,
        };
        if from.is_terminal() {
            // Late duplicates (a poll after the user channel already closed
            // the order) are no-ops, never re-opens or re-fills.
            self.updated_at = obs.at;
            return Ok(effect);
        }

        // 1. Fill delta from the cumulative matched size.
        if let Some(trade_id) = obs.trade_id.as_deref() {
            if self.booked_trade_ids.iter().any(|t| t == trade_id) {
                self.updated_at = obs.at;
                return Ok(effect);
            }
        }
        let mut pending_trade_matched: Option<f64> = None;
        let cumulative = match (obs.size_matched, obs.fill_delta) {
            (Some(matched), _) => Some(matched),
            (None, Some(delta)) => {
                if !delta.is_finite() || delta < 0.0 {
                    return Err(PolyError::lifecycle(format!(
                        "trade fill size {delta} invalid for {}",
                        self.venue_order_id
                    )));
                }
                // A per-trade delta is reconciled against the cumulative the
                // venue already reported: the trade may be one a poll has
                // already captured (its event arrived late or after a
                // reconnect), so the candidate cumulative is the sum of the
                // distinct trade deltas seen so far — not `size_matched +
                // delta` — and only its excess over `size_matched` is new.
                let trade_matched = self.trade_matched + delta;
                pending_trade_matched = Some(trade_matched);
                Some(trade_matched.max(self.size_matched))
            }
            (None, None) => None,
        };
        if let Some(matched) = cumulative {
            if !matched.is_finite() || matched < 0.0 {
                return Err(PolyError::lifecycle(format!(
                    "venue matched size {matched} invalid for {}",
                    self.venue_order_id
                )));
            }
            if matched > self.size_tokens + 1e-6 {
                return Err(PolyError::lifecycle(format!(
                    "venue matched {matched} > order size {} for {}",
                    self.size_tokens, self.venue_order_id
                )));
            }
            if let Some(t) = pending_trade_matched {
                self.trade_matched = t;
            }
            for t in &obs.associate_trades {
                self.remember_trade(t);
            }
            if obs.size_matched.is_some() && !obs.associate_trades.is_empty() {
                // Every trade behind this cumulative is now known by id, so
                // any later trade event that is NOT a no-op is genuinely new:
                // re-base the per-trade sum to the venue's cumulative.
                self.trade_matched = self.trade_matched.max(matched);
            }
            let delta = matched - self.size_matched;
            if delta > 1e-9 {
                effect.fill_delta = delta;
                self.size_matched = matched.min(self.size_tokens);
                if let Some(t) = obs.trade_id.clone() {
                    self.booked_trade_ids.push(t.clone());
                    effect.trade_id = Some(t);
                }
            } else if let Some(t) = obs.trade_id.clone() {
                // A trade id that adds nothing (already covered by a poll)
                // is still remembered so a later replay stays a no-op.
                self.booked_trade_ids.push(t);
            }
        }

        // 2. Target local state.
        let fully = self.size_matched >= self.size_tokens - 1e-6;
        let to = match obs.state {
            VenueOrderState::Matched => {
                if fully {
                    LocalOrderState::Filled
                } else if self.is_fill_and_kill() {
                    // FAK: `matched` means "done", not "everything matched"
                    // — whatever did not match was killed, never rested.
                    // With an explicit quantity the order closes at exactly
                    // what the venue reported (nothing matched → the venue
                    // killed all of it). Without one (the `POST /order`
                    // answer is status-only) nothing is booked and the
                    // order stays open until a poll, a trade event or
                    // reconciliation reports the quantity. The killed
                    // remainder is never fabricated into a fill.
                    match cumulative {
                        Some(_) if self.size_matched > 1e-9 => LocalOrderState::Filled,
                        Some(_) => LocalOrderState::Cancelled,
                        None => from,
                    }
                } else {
                    // GTC / GTD / FOK: the venue reports `matched` only once
                    // the whole order matched (a GTC/GTD with a partial fill
                    // stays `live`; FOK is all-or-nothing), so a status-only
                    // or lagging `matched` means the order size is the fill.
                    let delta = self.size_tokens - self.size_matched;
                    if delta > 1e-9 {
                        effect.fill_delta += delta;
                        self.size_matched = self.size_tokens;
                    }
                    LocalOrderState::Filled
                }
            }
            VenueOrderState::Cancelled => LocalOrderState::Cancelled,
            VenueOrderState::Expired => LocalOrderState::Expired,
            VenueOrderState::Live | VenueOrderState::Delayed | VenueOrderState::Unmatched => {
                if fully {
                    LocalOrderState::Filled
                } else if self.size_matched > 1e-9 {
                    LocalOrderState::PartiallyFilled
                } else {
                    LocalOrderState::Resting
                }
            }
            VenueOrderState::Unknown => {
                if fully {
                    LocalOrderState::Filled
                } else if self.size_matched > 1e-9 {
                    LocalOrderState::PartiallyFilled
                } else {
                    LocalOrderState::Unknown
                }
            }
        };

        self.venue_status = obs.raw_status.clone();
        self.updated_at = obs.at;
        if to != from {
            if !from.can_transition_to(to) {
                return Err(PolyError::lifecycle(format!(
                    "illegal transition {} -> {} for {}",
                    from.as_str(),
                    to.as_str(),
                    self.venue_order_id
                )));
            }
            self.state = to;
            effect.to = to;
            effect.transitioned = true;
        }
        Ok(effect)
    }

    /// Force a local terminal state without a venue observation (our own
    /// cancel confirmed, a definite failure, a paper fill).
    pub fn force_state(
        &mut self,
        to: LocalOrderState,
        raw_status: &str,
        at: DateTime<Utc>,
    ) -> PolyResult<bool> {
        if self.state == to {
            return Ok(false);
        }
        if !self.state.can_transition_to(to) {
            return Err(PolyError::lifecycle(format!(
                "illegal transition {} -> {} for {}",
                self.state.as_str(),
                to.as_str(),
                self.venue_order_id
            )));
        }
        self.state = to;
        self.venue_status = raw_status.to_string();
        self.updated_at = at;
        Ok(true)
    }

    /// Deterministic fill id for a fill without a venue trade id:
    /// `pfill_` + digest of (venue order, cumulative matched after the fill,
    /// source) — the same poll delta booked twice collides on purpose.
    pub fn fill_id(&self, trade_id: Option<&str>, source: FillSource) -> String {
        match trade_id {
            Some(t) if !t.trim().is_empty() => format!("trade:{}", t.trim()),
            _ => {
                let digest = bot_core::auth::sha256_hex(&format!(
                    "pfill|{}|{:.6}|{}",
                    self.venue_order_id,
                    self.size_matched,
                    source.as_str()
                ));
                format!("pfill_{}", &digest[..32])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_rounding_configs() {
        assert_eq!(RoundConfig::for_tick("0.1").unwrap().amount, 3);
        assert_eq!(RoundConfig::for_tick("0.01").unwrap().price, 2);
        assert_eq!(RoundConfig::for_tick("0.0001").unwrap().amount, 6);
        assert!(RoundConfig::for_tick("0.5").is_err());
    }

    #[test]
    fn buy_amounts_scale_to_six_decimals() {
        // Buy 10 tokens at 0.50 with a 0.01 tick: USDC in = 5.0, tokens out = 10.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(true, 10.0, 0.50, cfg);
        assert_eq!(a.side, SIDE_BUY);
        assert_eq!(a.maker_amount, 5_000_000); // 5 USDC
        assert_eq!(a.taker_amount, 10_000_000); // 10 tokens
    }

    #[test]
    fn sell_amounts_scale_to_six_decimals() {
        // Sell 10 tokens at 0.60: tokens in = 10, USDC out = 6.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(false, 10.0, 0.60, cfg);
        assert_eq!(a.side, SIDE_SELL);
        assert_eq!(a.maker_amount, 10_000_000); // 10 tokens
        assert_eq!(a.taker_amount, 6_000_000); // 6 USDC
    }

    #[test]
    fn price_is_rounded_to_the_tick() {
        // 0.01 tick => price rounded to 2 dp: 0.567 -> 0.57.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(true, 1.0, 0.567, cfg);
        // 1 token * 0.57 = 0.57 USDC.
        assert_eq!(a.taker_amount, 1_000_000);
        assert_eq!(a.maker_amount, 570_000);
    }

    #[test]
    fn sign_bundle_recovers_and_uses_neg_risk_contract() {
        let key_bytes =
            hex::decode("4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318")
                .unwrap();
        let key = SigningKey::from_slice(&key_bytes).unwrap();
        let params = OrderParams {
            token_id: "123456789",
            is_buy: true,
            size: 5.0,
            price: 0.4,
            tick_size: "0.01",
            neg_risk: true,
            chain_id: 137,
            domain_version: "2",
            exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
            neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
            signature_type: 0,
            funder: None,
            expiration_timestamp: 0,
            builder_code: None,
        };
        let bundle = sign_order_bundle(&key, &params).unwrap();
        // neg_risk => the neg-risk exchange verifies.
        assert_eq!(
            bundle.verifying_contract,
            "0xe2222d279d744050d28e00520010520000310F59"
        );
        assert!(bundle.signature.starts_with("0x"));
        // EOA signature is 65 bytes => 130 hex chars + "0x".
        assert_eq!(bundle.signature.len(), 2 + 130);
        assert_eq!(bundle.order.side, SIDE_BUY);
        assert_eq!(bundle.order.maker_amount, "2000000"); // 5 * 0.4 = 2 USDC
        assert_eq!(bundle.order.taker_amount, "5000000"); // 5 tokens
    }

    fn test_params<'a>(size: f64, expiration: u64) -> OrderParams<'a> {
        OrderParams {
            token_id: "12345678901234567890",
            is_buy: true,
            size,
            price: 0.5,
            tick_size: "0.01",
            neg_risk: false,
            chain_id: 137,
            domain_version: "2",
            exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
            neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
            signature_type: 0,
            funder: None,
            expiration_timestamp: expiration,
            builder_code: None,
        }
    }

    #[test]
    fn eoa_signing_refuses_a_funder_that_is_not_the_signer() {
        let key = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let own = eip712::address_from_signing_key(&key);
        let foreign = "0x000000000000000000000000000000000000dEaD";

        // Type 0 with a foreign funder: refused before anything is signed.
        let mut p = test_params(10.0, 0);
        p.funder = Some(foreign);
        let err = sign_order_bundle(&key, &p).unwrap_err().to_string();
        assert!(err.contains("signature_type 0"), "{err}");
        assert!(err.contains(foreign), "{err}");

        // Type 0 with the signer itself as funder (any case) is fine and the
        // maker is the signer.
        let upper = own.to_uppercase().replacen("0X", "0x", 1);
        let mut p = test_params(10.0, 0);
        p.funder = Some(&upper);
        let b = sign_order_bundle(&key, &p).unwrap();
        assert!(b.order.maker.eq_ignore_ascii_case(&own));

        // Proxy / safe types carry a foreign maker by design.
        for st in 1u8..=2 {
            let mut p = test_params(10.0, 0);
            p.signature_type = st;
            p.funder = Some(foreign);
            let b = sign_order_bundle(&key, &p).unwrap();
            assert_eq!(b.order.maker, foreign);
            assert_eq!(b.order.signer, own);
        }
    }

    #[test]
    fn derived_order_id_is_deterministic_per_intent() {
        // §G/§Q: re-signing the SAME intent (e.g. after an ambiguous HTTP
        // timeout or a restart) must produce the SAME CLOB order id so the
        // venue deduplicates instead of resting a second live order.
        let key = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let a = sign_order_bundle(&key, &test_params(10.0, 0)).unwrap();
        let b = sign_order_bundle(&key, &test_params(10.0, 0)).unwrap();
        assert_eq!(a.derived_order_id().unwrap(), b.derived_order_id().unwrap());
        // Different size → different intent → different id.
        let c = sign_order_bundle(&key, &test_params(11.0, 0)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), c.derived_order_id().unwrap());
        // GTD: a new expiration window is a new intent.
        let d = sign_order_bundle(&key, &test_params(10.0, 1_900_000_000)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), d.derived_order_id().unwrap());
        // Another signer → different id (identity is part of the salt).
        let key2 = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let e = sign_order_bundle(&key2, &test_params(10.0, 0)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), e.derived_order_id().unwrap());
        // Format: 0x + 64 hex chars (keccak256).
        let id = a.derived_order_id().unwrap();
        assert!(id.starts_with("0x") && id.len() == 66, "{id}");
    }

    // ------------------------------------------------------------------
    // TASK 4: signal identity, vocabulary and the lifecycle machine
    // ------------------------------------------------------------------

    fn decision(price: f64, size: f64) -> OrderDecision {
        OrderDecision {
            token_id: "111".into(),
            outcome: "Yes".into(),
            is_buy: true,
            size_tokens: size,
            limit_price: price,
            stake_usd: price * size,
            condition_id: "0xCOND".into(),
            neg_risk: false,
            reason: "test".into(),
        }
    }

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn signal(price: f64, size: f64) -> OrderSignal {
        OrderSignal::new(
            decision(price, size),
            "value",
            ExecutionMode::Paper,
            "GTC",
            "0.01",
            0,
            "Will it?",
            t0(),
        )
    }

    fn tracked(size: f64) -> TrackedOrder {
        TrackedOrder {
            venue_order_id: "0xorder".into(),
            order_id: "ord_1".into(),
            signal_id: "psig_1".into(),
            condition_id: "0xcond".into(),
            token_id: "111".into(),
            outcome: "Yes".into(),
            question: "Will it?".into(),
            is_buy: true,
            neg_risk: false,
            order_type: "GTC".into(),
            limit_price: 0.40,
            size_tokens: size,
            size_matched: 0.0,
            mode: ExecutionMode::Live,
            state: LocalOrderState::Submitted,
            venue_status: String::new(),
            expiration: 0,
            position_id: None,
            signature: Some("0xsig".into()),
            submitted_at: t0(),
            updated_at: t0(),
            booked_trade_ids: Vec::new(),
            trade_matched: 0.0,
        }
    }

    fn obs(state: VenueOrderState, matched: Option<f64>, trade: Option<&str>) -> VenueObservation {
        VenueObservation {
            state,
            raw_status: state.as_str().to_string(),
            size_matched: matched,
            fill_delta: None,
            trade_id: trade.map(str::to_string),
            price: None,
            source: FillSource::Poll,
            at: t0(),
            associate_trades: Vec::new(),
        }
    }

    #[test]
    fn trade_deltas_accumulate_and_replays_are_ignored() {
        let mut o = tracked(10.0);
        o.apply_venue(&obs(VenueOrderState::Live, Some(0.0), None))
            .unwrap();
        let mut ev = obs(VenueOrderState::Live, None, Some("t1"));
        ev.fill_delta = Some(2.5);
        ev.price = Some(0.39);
        ev.source = FillSource::UserWs;
        let e = o.apply_venue(&ev).unwrap();
        assert!((e.fill_delta - 2.5).abs() < 1e-9);
        assert_eq!(o.state, LocalOrderState::PartiallyFilled);
        // Replay of the same trade id: nothing.
        assert_eq!(o.apply_venue(&ev).unwrap().fill_delta, 0.0);
        // A poll that already covers a trade keeps the trade id remembered.
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(6.0), None))
            .unwrap();
        assert!((e.fill_delta - 3.5).abs() < 1e-9);
        let mut t2 = obs(VenueOrderState::Live, None, Some("t2"));
        t2.fill_delta = Some(0.0);
        assert_eq!(o.apply_venue(&t2).unwrap().fill_delta, 0.0);
        assert!(o.booked_trade_ids.contains(&"t2".to_string()));
        // A trade arriving after a poll already captured part of the fill
        // books only the excess of the distinct-trade sum over the reported
        // cumulative (2.5 + 5.0 = 7.5 vs 6.0 → 1.5), never `6.0 + 5.0`.
        let mut late = obs(VenueOrderState::Live, None, Some("t3"));
        late.fill_delta = Some(5.0);
        let e = o.apply_venue(&late).unwrap();
        assert!((e.fill_delta - 1.5).abs() < 1e-9);
        assert!((o.size_matched - 7.5).abs() < 1e-9);
        assert!((o.trade_matched - 7.5).abs() < 1e-9);
        // A distinct-trade sum beyond the order size is an invariant
        // violation, and a refused trade is not accumulated.
        let mut big = obs(VenueOrderState::Live, None, Some("t4"));
        big.fill_delta = Some(4.0);
        assert!(matches!(o.apply_venue(&big), Err(PolyError::Lifecycle(_))));
        assert!((o.size_matched - 7.5).abs() < 1e-9);
        assert!((o.trade_matched - 7.5).abs() < 1e-9);
        assert!(!o.booked_trade_ids.contains(&"t4".to_string()));
        // Exactly the remainder fills the order.
        let mut last = obs(VenueOrderState::Live, None, Some("t5"));
        last.fill_delta = Some(2.5);
        let e = o.apply_venue(&last).unwrap();
        assert_eq!(e.to, LocalOrderState::Filled);
        assert!((e.fill_delta - 2.5).abs() < 1e-9);
    }

    #[test]
    fn late_trade_events_after_a_poll_are_never_booked_twice() {
        // The poll captured the fill first (the user channel was
        // reconnecting): the late trade event adds nothing.
        let mut o = tracked(10.0);
        o.apply_venue(&obs(VenueOrderState::Live, Some(6.0), None))
            .unwrap();
        let mut t1 = obs(VenueOrderState::Live, None, Some("t-1"));
        t1.fill_delta = Some(6.0);
        t1.source = FillSource::UserWs;
        let e = o.apply_venue(&t1).unwrap();
        assert_eq!(e.fill_delta, 0.0, "late event for a poll-captured trade");
        assert!((o.size_matched - 6.0).abs() < 1e-9);
        assert!(o.booked_trade_ids.contains(&"t-1".to_string()));
        // A genuinely new trade after that books in full.
        let mut t2 = obs(VenueOrderState::Live, None, Some("t-2"));
        t2.fill_delta = Some(4.0);
        let e = o.apply_venue(&t2).unwrap();
        assert!((e.fill_delta - 4.0).abs() < 1e-9);
        assert_eq!(o.state, LocalOrderState::Filled);

        // The poll captured two trades (2 + 4) but only one event is ever
        // seen: a third trade books only the excess (conservative) and the
        // next poll converges — under-booking briefly, never over-booking.
        let mut o = tracked(10.0);
        o.apply_venue(&obs(VenueOrderState::Live, Some(6.0), None))
            .unwrap();
        let mut t1 = obs(VenueOrderState::Live, None, Some("t-1"));
        t1.fill_delta = Some(4.0);
        assert_eq!(o.apply_venue(&t1).unwrap().fill_delta, 0.0);
        let mut t3 = obs(VenueOrderState::Live, None, Some("t-3"));
        t3.fill_delta = Some(3.0);
        let e = o.apply_venue(&t3).unwrap();
        assert!((e.fill_delta - 1.0).abs() < 1e-9, "4 + 3 = 7 vs 6 → 1");
        assert!((o.size_matched - 7.0).abs() < 1e-9);
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(9.0), None))
            .unwrap();
        assert!((e.fill_delta - 2.0).abs() < 1e-9);
        assert!((o.size_matched - 9.0).abs() < 1e-9);

        // A cumulative that names its trades (`associate_trades`) remembers
        // them and re-bases the per-trade sum, so after a restart (adopted
        // cumulative, no trade memory) a NEW trade books in full at once
        // while the named ones stay no-ops.
        let mut o = tracked(10.0);
        o.size_matched = 6.0;
        o.state = LocalOrderState::PartiallyFilled;
        let mut poll = obs(VenueOrderState::Live, Some(6.0), None);
        poll.associate_trades = vec!["t-0".into(), "t-1".into()];
        assert_eq!(o.apply_venue(&poll).unwrap().fill_delta, 0.0);
        assert!(o.booked_trade_ids.contains(&"t-0".to_string()));
        assert!(o.booked_trade_ids.contains(&"t-1".to_string()));
        assert!((o.trade_matched - 6.0).abs() < 1e-9);
        let mut t1 = obs(VenueOrderState::Live, None, Some("t-1"));
        t1.fill_delta = Some(4.0);
        assert_eq!(o.apply_venue(&t1).unwrap().fill_delta, 0.0, "named trade");
        let mut t2 = obs(VenueOrderState::Live, None, Some("t-2"));
        t2.fill_delta = Some(3.0);
        let e = o.apply_venue(&t2).unwrap();
        assert!((e.fill_delta - 3.0).abs() < 1e-9);
        assert!((o.size_matched - 9.0).abs() < 1e-9);

        // Without named trades an adopted order's new trades wait for the
        // poll: the delta is remembered but not booked, the poll books it.
        let mut o = tracked(10.0);
        o.size_matched = 6.0;
        o.state = LocalOrderState::PartiallyFilled;
        let mut t2 = obs(VenueOrderState::Live, None, Some("t-2"));
        t2.fill_delta = Some(3.0);
        assert_eq!(o.apply_venue(&t2).unwrap().fill_delta, 0.0);
        assert!(o.booked_trade_ids.contains(&"t-2".to_string()));
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(9.0), None))
            .unwrap();
        assert!((e.fill_delta - 3.0).abs() < 1e-9);
        assert_eq!(o.apply_venue(&t2).unwrap().fill_delta, 0.0, "replay");
    }

    #[test]
    fn intent_key_is_deterministic_and_semantic() {
        let a = signal(0.40, 10.0);
        let b = signal(0.40, 10.0);
        assert_eq!(a.intent_key(), b.intent_key());
        assert_eq!(a.signal_id, b.signal_id);
        assert!(a.signal_id.starts_with("psig_"));
        assert_eq!(a.signal_id.len(), 5 + 32);
        // Tick rounding: 0.4001 and 0.40 share a key on a 0.01 grid…
        let c = OrderSignal::new(
            decision(0.4001, 10.0),
            "value",
            ExecutionMode::Paper,
            "gtc",
            "0.01",
            0,
            "other question",
            t0() + chrono::Duration::hours(1),
        );
        assert_eq!(
            a.intent_key(),
            c.intent_key(),
            "created_at/question/case never matter"
        );
        // …but not on a 0.0001 grid.
        let d = OrderSignal::new(
            decision(0.4001, 10.0),
            "value",
            ExecutionMode::Paper,
            "GTC",
            "0.0001",
            0,
            "q",
            t0(),
        );
        assert_ne!(a.intent_key(), d.intent_key());
        // Size, price, side, expiry, order type and mode all change the key.
        assert_ne!(a.intent_key(), signal(0.41, 10.0).intent_key());
        assert_ne!(a.intent_key(), signal(0.40, 10.5).intent_key());
        let mut sell = decision(0.40, 10.0);
        sell.is_buy = false;
        let sell = OrderSignal::new(
            sell,
            "value",
            ExecutionMode::Paper,
            "GTC",
            "0.01",
            0,
            "q",
            t0(),
        );
        assert_ne!(a.intent_key(), sell.intent_key());
        assert_eq!(sell.side_str(), "sell");
        let gtd = OrderSignal::new(
            decision(0.40, 10.0),
            "value",
            ExecutionMode::Paper,
            "GTD",
            "0.01",
            1_800_000_000,
            "q",
            t0(),
        );
        assert_ne!(a.intent_key(), gtd.intent_key());
        let live = OrderSignal::new(
            decision(0.40, 10.0),
            "value",
            ExecutionMode::Live,
            "GTC",
            "0.01",
            0,
            "q",
            t0(),
        );
        assert_ne!(a.intent_key(), live.intent_key());
        assert!(a.paper_order_id().starts_with("paper:"));
        assert_eq!(a.paper_order_id(), b.paper_order_id());
        assert_eq!(round_price_to_tick(0.4567, "0.01"), 0.46);
        assert_eq!(round_price_to_tick(0.4567, "0.001"), 0.457);
        assert_eq!(round_price_to_tick(0.4567, "bogus"), 0.46);
    }

    #[test]
    fn stage_and_reject_vocabularies_are_stable_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in PolyStage::ALL {
            assert!(seen.insert(s.as_str()), "dup {s}");
        }
        assert_eq!(seen.len(), 17);
        assert!(PolyStage::Filled.is_terminal());
        assert!(PolyStage::Resting.is_terminal());
        assert!(PolyStage::Ambiguous.is_terminal());
        assert!(!PolyStage::Submitted.is_terminal());
        assert!(PolyStage::Received < PolyStage::Submitted);
        let mut seen = std::collections::HashSet::new();
        for r in RejectReason::ALL {
            assert!(seen.insert(r.as_str()), "dup {r}");
            assert!(r
                .as_str()
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_'));
        }
        assert_eq!(seen.len(), 36);
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::PolyEmergencyDisabled)),
            RejectReason::EmergencyDisabled
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::PolyDailyLoss)),
            RejectReason::DailyLossLimit
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::PolyOpenOrderCap)),
            RejectReason::OpenOrderCap
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::PolyMarketExposure)),
            RejectReason::ExposureCap
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::DuplicateSymbol)),
            RejectReason::AlreadyInMarket
        );
        assert_eq!(
            RejectReason::from_risk_code(Some(RiskCode::ReentryCooldown)),
            RejectReason::RiskRejected
        );
        assert_eq!(
            RejectReason::from_risk_code(None),
            RejectReason::RiskRejected
        );
        assert_eq!(
            RejectReason::from_skip(crate::strategy::SkipReason::StaleQuote),
            RejectReason::StaleQuote
        );
        assert_eq!(
            RejectReason::from_skip(crate::strategy::SkipReason::MarketClosed),
            RejectReason::MarketNotTradeable
        );
        assert_eq!(
            RejectReason::from_error(&PolyError::http("reset")),
            RejectReason::SubmitUnknown
        );
        assert_eq!(
            RejectReason::from_error(&PolyError::clob("invalid amounts")),
            RejectReason::VenueRejected
        );
        assert_eq!(
            RejectReason::from_error(&PolyError::insufficient_funding("x")),
            RejectReason::InsufficientFunding
        );
        for s in [
            "live",
            "LIVE",
            "matched",
            "delayed",
            "unmatched",
            "canceled",
            "cancelled",
            "expired",
            "???",
        ] {
            let parsed = VenueOrderState::parse(s);
            assert_eq!(VenueOrderState::parse(parsed.as_str()), parsed, "{s}");
        }
        assert_eq!(VenueOrderState::parse("filled"), VenueOrderState::Matched);
        assert!(VenueOrderState::Matched.is_terminal());
        assert!(!VenueOrderState::Delayed.is_terminal());
        for l in [
            LocalOrderState::Submitted,
            LocalOrderState::Resting,
            LocalOrderState::PartiallyFilled,
            LocalOrderState::Filled,
            LocalOrderState::Cancelled,
            LocalOrderState::Expired,
            LocalOrderState::Unknown,
            LocalOrderState::Failed,
        ] {
            assert_eq!(LocalOrderState::parse(l.as_str()), l);
            // The OMS status mapping must itself be a legal OMS state.
            let _ = l.oms_status();
        }
        assert_eq!(LocalOrderState::Resting.oms_status(), OrderStatus::Accepted);
        assert_eq!(
            LocalOrderState::PartiallyFilled.oms_status(),
            OrderStatus::PartiallyFilled
        );
        assert!(!LocalOrderState::Filled.can_transition_to(LocalOrderState::Resting));
        assert!(!LocalOrderState::Resting.can_transition_to(LocalOrderState::Submitted));
        assert!(LocalOrderState::Unknown.can_transition_to(LocalOrderState::Filled));
        assert!(!LocalOrderState::PartiallyFilled.can_transition_to(LocalOrderState::Resting));
    }

    #[test]
    fn lifecycle_books_partial_fills_once_and_ends_terminal() {
        let mut o = tracked(10.0);
        // Accepted, nothing matched.
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(0.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!(e.transitioned);
        assert_eq!(o.state, LocalOrderState::Resting);
        assert!((o.resting_quote() - 4.0).abs() < 1e-9);
        // Partial fill 4 of 10.
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(4.0), None))
            .unwrap();
        assert!((e.fill_delta - 4.0).abs() < 1e-9);
        assert_eq!(o.state, LocalOrderState::PartiallyFilled);
        assert!((o.remaining() - 6.0).abs() < 1e-9);
        // Same poll again: nothing new (no double booking).
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(4.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!(!e.transitioned);
        // A stale poll (lower matched) never un-fills.
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(2.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!((o.size_matched - 4.0).abs() < 1e-9);
        // User-channel trade event with an id: booked once.
        let mut ev = obs(VenueOrderState::Live, Some(7.0), Some("trade-1"));
        ev.source = FillSource::UserWs;
        ev.price = Some(0.39);
        let e = o.apply_venue(&ev).unwrap();
        assert!((e.fill_delta - 3.0).abs() < 1e-9);
        assert!((e.fill_price - 0.39).abs() < 1e-9);
        assert_eq!(e.trade_id.as_deref(), Some("trade-1"));
        let e = o.apply_venue(&ev).unwrap();
        assert_eq!(e.fill_delta, 0.0, "replayed trade id is ignored");
        assert_eq!(
            o.fill_id(Some("trade-1"), FillSource::UserWs),
            "trade:trade-1"
        );
        let f1 = o.fill_id(None, FillSource::Poll);
        assert!(f1.starts_with("pfill_"));
        assert_eq!(f1, o.fill_id(None, FillSource::Poll));
        // Matched → filled, remaining size booked.
        let e = o
            .apply_venue(&obs(VenueOrderState::Matched, None, None))
            .unwrap();
        assert!((e.fill_delta - 3.0).abs() < 1e-9);
        assert_eq!(o.state, LocalOrderState::Filled);
        assert_eq!(o.resting_quote(), 0.0);
        // Terminal: later observations are no-ops.
        let e = o
            .apply_venue(&obs(VenueOrderState::Live, Some(0.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!(!e.transitioned);
        assert_eq!(o.state, LocalOrderState::Filled);
    }

    #[test]
    fn lifecycle_handles_cancel_expire_unknown_and_invariants() {
        let mut o = tracked(10.0);
        o.apply_venue(&obs(VenueOrderState::Live, Some(3.0), None))
            .unwrap();
        let e = o
            .apply_venue(&obs(VenueOrderState::Cancelled, Some(3.0), None))
            .unwrap();
        assert_eq!(e.to, LocalOrderState::Cancelled);
        assert!(
            (o.size_matched - 3.0).abs() < 1e-9,
            "partial fill stays booked"
        );
        assert_eq!(o.resting_quote(), 0.0);

        let mut o = tracked(10.0);
        let e = o
            .apply_venue(&obs(VenueOrderState::Expired, None, None))
            .unwrap();
        assert_eq!(e.to, LocalOrderState::Expired);

        let mut o = tracked(10.0);
        let e = o
            .apply_venue(&obs(VenueOrderState::Unknown, None, None))
            .unwrap();
        assert_eq!(e.to, LocalOrderState::Unknown);
        // Unknown resolves once the venue answers.
        let e = o
            .apply_venue(&obs(VenueOrderState::Matched, Some(10.0), None))
            .unwrap();
        assert_eq!(e.to, LocalOrderState::Filled);
        assert!((e.fill_delta - 10.0).abs() < 1e-9);

        // Over-fill and negative matched are lifecycle errors, not fills.
        let mut o = tracked(10.0);
        assert!(matches!(
            o.apply_venue(&obs(VenueOrderState::Live, Some(11.0), None)),
            Err(PolyError::Lifecycle(_))
        ));
        assert!(matches!(
            o.apply_venue(&obs(VenueOrderState::Live, Some(-1.0), None)),
            Err(PolyError::Lifecycle(_))
        ));
        assert_eq!(o.size_matched, 0.0);

        // force_state respects the machine.
        let mut o = tracked(10.0);
        assert!(o
            .force_state(LocalOrderState::Cancelled, "cancel", t0())
            .unwrap());
        assert!(!o
            .force_state(LocalOrderState::Cancelled, "cancel", t0())
            .unwrap());
        assert!(matches!(
            o.force_state(LocalOrderState::Resting, "x", t0()),
            Err(PolyError::Lifecycle(_))
        ));

        // TTL / expiry / reprice helpers.
        let o = tracked(10.0);
        assert!(!o.ttl_elapsed(t0() + chrono::Duration::seconds(59), 60));
        assert!(o.ttl_elapsed(t0() + chrono::Duration::seconds(60), 60));
        assert!(!o.ttl_elapsed(t0() + chrono::Duration::hours(9), 0));
        let mut g = tracked(10.0);
        g.expiration = t0().timestamp() as u64 + 10;
        assert!(!g.expiry_passed(t0()));
        assert!(g.expiry_passed(t0() + chrono::Duration::seconds(11)));
        assert!(o.needs_reprice(0.45, 0.02));
        assert!(!o.needs_reprice(0.41, 0.02));
        assert!(!o.needs_reprice(0.45, 0.0));
        assert!(!o.needs_reprice(0.0, 0.02));
        assert_eq!(o.age_secs(t0() + chrono::Duration::seconds(5)), 5);
    }

    #[test]
    fn fak_matched_never_fabricates_the_killed_remainder() {
        // GTC (and GTD/FOK): a `matched` that arrives with a lagging or
        // absent quantity means the whole order matched — pinned.
        let mut gtc = tracked(10.0);
        let e = gtc
            .apply_venue(&obs(VenueOrderState::Matched, Some(7.0), None))
            .unwrap();
        assert!((e.fill_delta - 10.0).abs() < 1e-9);
        assert_eq!(gtc.state, LocalOrderState::Filled);

        // FAK, status-only `matched` (the POST answer): nothing is booked,
        // the order stays open and is marked venue-acknowledged so a later
        // `404` is treated as "vanished", never as "never existed".
        let mut fak = tracked(10.0);
        fak.order_type = "FAK".into();
        assert!(fak.is_fill_and_kill());
        assert!(!fak.venue_acknowledged());
        let e = fak
            .apply_venue(&obs(VenueOrderState::Matched, None, None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!(!e.transitioned);
        assert_eq!(fak.state, LocalOrderState::Submitted);
        assert_eq!(fak.venue_status, "matched");
        assert!(fak.venue_acknowledged());
        assert_eq!(fak.size_matched, 0.0);

        // The venue then reports the real quantity: only that is booked, the
        // killed remainder is never a fill, and the order is terminal.
        let e = fak
            .apply_venue(&obs(VenueOrderState::Matched, Some(7.0), None))
            .unwrap();
        assert!((e.fill_delta - 7.0).abs() < 1e-9);
        assert_eq!(e.to, LocalOrderState::Filled);
        assert!((fak.size_matched - 7.0).abs() < 1e-9);
        assert!((fak.remaining() - 3.0).abs() < 1e-9);
        assert_eq!(fak.resting_quote(), 0.0, "terminal: nothing committed");
        // Late status-only `matched` on the terminal order: still nothing.
        let e = fak
            .apply_venue(&obs(VenueOrderState::Matched, None, None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert!((fak.size_matched - 7.0).abs() < 1e-9);

        // A per-trade delta (user channel) resolves the quantity the same
        // way, and the subsequent cumulative `matched` adds nothing.
        let mut fak = tracked(10.0);
        fak.order_type = "FAK".into();
        fak.apply_venue(&obs(VenueOrderState::Matched, None, None))
            .unwrap();
        let mut trade = obs(VenueOrderState::Live, None, Some("t-9"));
        trade.fill_delta = Some(4.0);
        trade.source = FillSource::UserWs;
        let e = fak.apply_venue(&trade).unwrap();
        assert!((e.fill_delta - 4.0).abs() < 1e-9);
        assert_eq!(fak.state, LocalOrderState::PartiallyFilled);
        let e = fak
            .apply_venue(&obs(VenueOrderState::Matched, Some(4.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert_eq!(fak.state, LocalOrderState::Filled);
        assert!((fak.size_matched - 4.0).abs() < 1e-9);

        // FAK `matched` with an explicit zero quantity: the venue killed all
        // of it — cancelled, nothing booked.
        let mut fak = tracked(10.0);
        fak.order_type = "FAK".into();
        let e = fak
            .apply_venue(&obs(VenueOrderState::Matched, Some(0.0), None))
            .unwrap();
        assert_eq!(e.fill_delta, 0.0);
        assert_eq!(fak.state, LocalOrderState::Cancelled);

        // FOK is all-or-nothing: `matched` is the whole order.
        let mut fok = tracked(10.0);
        fok.order_type = "FOK".into();
        let e = fok
            .apply_venue(&obs(VenueOrderState::Matched, None, None))
            .unwrap();
        assert!((e.fill_delta - 10.0).abs() < 1e-9);
        assert_eq!(fok.state, LocalOrderState::Filled);

        // `remember_trade` dedups without booking.
        let mut o = tracked(10.0);
        o.remember_trade("t-1");
        o.remember_trade("t-1");
        o.remember_trade("  ");
        assert_eq!(o.booked_trade_ids, vec!["t-1".to_string()]);
        let mut replay = obs(VenueOrderState::Live, None, Some("t-1"));
        replay.fill_delta = Some(3.0);
        let e = o.apply_venue(&replay).unwrap();
        assert_eq!(e.fill_delta, 0.0, "remembered trade id is a no-op");
        assert_eq!(o.size_matched, 0.0);
    }

    #[test]
    fn signal_outcome_reports_venue_reach() {
        let mk = |stage| SignalOutcome {
            signal_id: "psig".into(),
            stage,
            reject_reason: None,
            detail: String::new(),
            order_id: None,
            venue_order_id: None,
            position_id: None,
            approved_stake: None,
            size_tokens: None,
            total_ms: 0,
        };
        assert!(mk(PolyStage::Filled).reached_venue());
        assert!(mk(PolyStage::Resting).reached_venue());
        assert!(mk(PolyStage::Ambiguous).reached_venue());
        assert!(!mk(PolyStage::Rejected).reached_venue());
        assert!(!mk(PolyStage::Failed).reached_venue());
        assert_eq!(FillSource::UserWs.as_str(), "user_ws");
    }
}
