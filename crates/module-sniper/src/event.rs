//! Unified launch event model (TASK 2 §B).
//!
//! Every detection source — PumpPortal, `logsSubscribe`, Geyser
//! `transactionSubscribe`, a replay fixture — is normalised into one
//! [`LaunchEvent`] before the pipeline sees it. The event carries:
//!
//! * a **deterministic event id** derived from the protocol, the mint, the
//!   pool and the on-chain reference (signature → slot → source sequence), so
//!   the same launch observed by two feeds maps to the same id;
//! * the **protocol** (pump.fun curve, PumpSwap pool, Raydium AMM v4 pool)
//!   and the **source** feed;
//! * on-chain identity (slot, signature, mint, creator, pool, base/quote);
//! * what the source knows about liquidity and the initial price;
//! * three timestamps (`event_ts` = chain/block time when known,
//!   `source_ts` = when the source emitted it, `observed_at` = when this
//!   process saw it) and the per-source sequence number used to detect
//!   reordering after a reconnect;
//! * a hash of the raw payload for audit/replay;
//! * the legacy [`TokenLaunch`] record (name, symbol, metadata URI, market
//!   cap, creator buy, socials) that the existing screening rules consume.
//!
//! The module is pure: no I/O, no clocks other than the caller-supplied
//! `now`, so every rule here is exercised by the replay system and by unit
//! tests without a network.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::models::{LaunchFeed, TokenLaunch, Venue};

/// Tolerated clock skew between a source/chain timestamp and our clock.
pub const MAX_CLOCK_SKEW_SECS: i64 = 5;
/// Length of an ed25519 signature; a `signature` that does not decode to
/// exactly this many bytes is malformed.
const SIGNATURE_LEN: usize = 64;

/// Which on-chain protocol produced the launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchProtocol {
    /// A pump.fun bonding-curve token creation (`Create` event).
    PumpFun,
    /// A PumpSwap AMM pool creation (`CreatePoolEvent`), including the
    /// canonical pool a pump.fun migration creates.
    PumpSwap,
    /// A Raydium AMM v4 pool initialisation (`initialize2`).
    RaydiumAmmV4,
}

impl LaunchProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            LaunchProtocol::PumpFun => "pump_fun",
            LaunchProtocol::PumpSwap => "pump_swap",
            LaunchProtocol::RaydiumAmmV4 => "raydium_amm_v4",
        }
    }

    /// The venue a position opened from this launch is booked under when
    /// the direct route is used.
    pub fn venue(&self) -> Venue {
        match self {
            LaunchProtocol::PumpFun => Venue::PumpFun,
            LaunchProtocol::PumpSwap => Venue::PumpSwap,
            LaunchProtocol::RaydiumAmmV4 => Venue::RaydiumAmmV4,
        }
    }

    /// AMM protocols need a pool address; the bonding curve is derived from
    /// the mint and may be omitted.
    pub fn requires_pool(&self) -> bool {
        !matches!(self, LaunchProtocol::PumpFun)
    }
}

impl std::fmt::Display for LaunchProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why an event failed shape validation. Each variant is a distinct,
/// testable malformation; the pipeline collapses them all into
/// `INVALID_EVENT` with the variant's text as the detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventDefect {
    EmptyEventId,
    InvalidMint(String),
    InvalidCreator(String),
    InvalidPool(String),
    MissingPool,
    BaseMintMismatch,
    InvalidQuoteMint(String),
    InvalidSignature(String),
    ZeroSlot,
    NonFiniteLiquidity,
    NonFinitePrice,
    ObservedInFuture,
    EventAfterObservation,
    EmptyRawHash,
    EventIdMismatch,
}

impl std::fmt::Display for EventDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventDefect::EmptyEventId => f.write_str("event_id is empty"),
            EventDefect::InvalidMint(m) => write!(f, "mint is not a pubkey: {m}"),
            EventDefect::InvalidCreator(c) => write!(f, "creator is not a pubkey: {c}"),
            EventDefect::InvalidPool(p) => write!(f, "pool is not a pubkey: {p}"),
            EventDefect::MissingPool => f.write_str("protocol requires a pool address"),
            EventDefect::BaseMintMismatch => f.write_str("base_mint differs from mint"),
            EventDefect::InvalidQuoteMint(q) => write!(f, "quote_mint is not a pubkey: {q}"),
            EventDefect::InvalidSignature(s) => write!(f, "signature is malformed: {s}"),
            EventDefect::ZeroSlot => f.write_str("slot is 0"),
            EventDefect::NonFiniteLiquidity => f.write_str("liquidity is not a finite number"),
            EventDefect::NonFinitePrice => f.write_str("initial price is not finite / negative"),
            EventDefect::ObservedInFuture => f.write_str("observed_at lies in the future"),
            EventDefect::EventAfterObservation => {
                f.write_str("event_ts is later than observed_at (beyond clock skew)")
            }
            EventDefect::EmptyRawHash => f.write_str("raw_hash is empty"),
            EventDefect::EventIdMismatch => {
                f.write_str("event_id does not match the event's identity fields")
            }
        }
    }
}

/// One normalised launch, regardless of protocol or feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchEvent {
    /// Deterministic id (see [`LaunchEvent::compute_event_id`]).
    pub event_id: String,
    pub protocol: LaunchProtocol,
    pub source: LaunchFeed,
    /// Slot of the creating transaction when the source reports it.
    #[serde(default)]
    pub slot: Option<u64>,
    /// Signature of the creating transaction when the source reports it.
    #[serde(default)]
    pub signature: Option<String>,
    /// The token being launched (base58).
    pub mint: String,
    /// Creator / pool initialiser wallet (base58; may be empty for sources
    /// that do not report it).
    #[serde(default)]
    pub creator: String,
    /// Bonding curve or AMM pool address. Required for AMM protocols.
    #[serde(default)]
    pub pool: Option<String>,
    /// Always the launched token for the protocols we support.
    pub base_mint: String,
    /// Always wrapped SOL for the protocols we support.
    pub quote_mint: String,
    /// Quote-side (SOL) liquidity the source reports, in lamports: the
    /// curve's initial real SOL or the pool's opening SOL deposit.
    #[serde(default)]
    pub liquidity_quote_lamports: Option<u64>,
    /// Base-side liquidity in raw token units, when the source reports it.
    #[serde(default)]
    pub liquidity_base_raw: Option<u64>,
    /// Opening price in SOL per whole token, when derivable.
    #[serde(default)]
    pub initial_price_sol: Option<f64>,
    /// Base token decimals when known (`None` until the mint is read).
    #[serde(default)]
    pub base_decimals: Option<u8>,
    /// On-chain / block time of the creation, when the source carries one.
    #[serde(default)]
    pub event_ts: Option<DateTime<Utc>>,
    /// When the source emitted the notification, when it says so.
    #[serde(default)]
    pub source_ts: Option<DateTime<Utc>>,
    /// When this process first saw the event.
    pub observed_at: DateTime<Utc>,
    /// Monotonic per-source sequence assigned by the detector (1-based).
    /// A lower sequence arriving after a higher one means the source
    /// reordered or replayed after a reconnect.
    #[serde(default)]
    pub source_seq: u64,
    /// Hex digest of the raw source payload (logs / JSON) for audit and
    /// replay matching. Never the payload itself.
    #[serde(default)]
    pub raw_hash: String,
    /// The legacy launch record consumed by screening, events and
    /// dashboards. Metadata (name/symbol/uri/socials/market cap) lives here.
    pub launch: TokenLaunch,
}

impl LaunchEvent {
    /// Build an event from a legacy [`TokenLaunch`] (pump.fun protocol,
    /// bonding-curve pool derived from the mint when it parses).
    pub fn from_token_launch(launch: TokenLaunch, source_seq: u64, raw_hash: String) -> Self {
        let pool = Pubkey::from_str(&launch.mint)
            .ok()
            .map(|m| solana_kit::pump::bonding_curve_pda(&m).to_string());
        let mut ev = LaunchEvent {
            event_id: String::new(),
            protocol: LaunchProtocol::PumpFun,
            source: launch.feed,
            slot: launch.slot,
            signature: launch.signature.clone(),
            mint: launch.mint.clone(),
            creator: launch.creator.clone(),
            pool,
            base_mint: launch.mint.clone(),
            quote_mint: solana_kit::consts::WSOL_MINT.to_string(),
            liquidity_quote_lamports: None,
            liquidity_base_raw: None,
            initial_price_sol: None,
            base_decimals: Some(6),
            event_ts: None,
            source_ts: None,
            observed_at: launch.observed_at,
            source_seq,
            raw_hash,
            launch,
        };
        ev.event_id = ev.compute_event_id();
        ev
    }

    /// Deterministic identity: protocol, mint, pool and the strongest
    /// available on-chain reference (signature, else slot, else the source
    /// sequence). Two feeds reporting the same creation transaction agree
    /// on the id; a manual/sequence-only event cannot collide with a real
    /// one because the reference kind is part of the digest.
    pub fn compute_event_id(&self) -> String {
        let reference = match (&self.signature, self.slot) {
            (Some(sig), _) if !sig.is_empty() => format!("sig:{sig}"),
            (_, Some(slot)) if slot > 0 => format!("slot:{slot}"),
            _ => format!("seq:{}:{}", self.source, self.source_seq),
        };
        let pool = self.pool.as_deref().unwrap_or("");
        let payload = format!(
            "launch|{}|{}|{}|{}",
            self.protocol.as_str(),
            self.mint,
            pool,
            reference
        );
        format!(
            "evt_{}",
            &bot_core::execution::digest_hex(payload.as_bytes())[..32]
        )
    }

    /// The one authoritative dedup key. pump.fun launches keep the legacy
    /// key (the mint) so a restart-safe dedup store populated before this
    /// change still recognises them; AMM launches are namespaced by
    /// protocol because a token can legitimately launch on the curve and
    /// later open a PumpSwap pool — two events, two decisions.
    pub fn dedup_key(&self) -> String {
        match self.protocol {
            LaunchProtocol::PumpFun => self.mint.clone(),
            other => format!("{}:{}", other.as_str(), self.mint),
        }
    }

    /// Best estimate of when the launch actually happened.
    pub fn effective_ts(&self) -> DateTime<Utc> {
        self.event_ts.or(self.source_ts).unwrap_or(self.observed_at)
    }

    /// Age of the launch at `now`, in milliseconds (never negative).
    pub fn age_ms(&self, now: DateTime<Utc>) -> u64 {
        now.signed_duration_since(self.effective_ts())
            .num_milliseconds()
            .max(0) as u64
    }

    /// Time this process has been holding the event, in milliseconds.
    pub fn held_ms(&self, now: DateTime<Utc>) -> u64 {
        now.signed_duration_since(self.observed_at)
            .num_milliseconds()
            .max(0) as u64
    }

    /// Stale when the launch is older than `max_age_secs` (`<= 0` = off).
    pub fn is_stale(&self, now: DateTime<Utc>, max_age_secs: i64) -> bool {
        if max_age_secs <= 0 {
            return false;
        }
        self.age_ms(now) > (max_age_secs as u64).saturating_mul(1_000)
    }

    /// Parsed mint (validated by [`LaunchEvent::validate_shape`]).
    pub fn mint_pubkey(&self) -> Option<Pubkey> {
        Pubkey::from_str(&self.mint).ok()
    }

    /// Parsed pool address, if any.
    pub fn pool_pubkey(&self) -> Option<Pubkey> {
        self.pool.as_deref().and_then(|p| Pubkey::from_str(p).ok())
    }

    /// Parsed creator, if reported.
    pub fn creator_pubkey(&self) -> Option<Pubkey> {
        Pubkey::from_str(&self.creator).ok()
    }

    /// Structural validation. Pure and total: every malformed shape maps
    /// to one [`EventDefect`], nothing panics on hostile input.
    pub fn validate_shape(&self, now: DateTime<Utc>) -> Result<(), EventDefect> {
        if self.event_id.trim().is_empty() {
            return Err(EventDefect::EmptyEventId);
        }
        if Pubkey::from_str(&self.mint).is_err() {
            return Err(EventDefect::InvalidMint(self.mint.clone()));
        }
        if !self.creator.is_empty() && Pubkey::from_str(&self.creator).is_err() {
            return Err(EventDefect::InvalidCreator(self.creator.clone()));
        }
        match &self.pool {
            Some(p) => {
                if Pubkey::from_str(p).is_err() {
                    return Err(EventDefect::InvalidPool(p.clone()));
                }
            }
            None if self.protocol.requires_pool() => return Err(EventDefect::MissingPool),
            None => {}
        }
        if self.base_mint != self.mint {
            return Err(EventDefect::BaseMintMismatch);
        }
        if Pubkey::from_str(&self.quote_mint).is_err() {
            return Err(EventDefect::InvalidQuoteMint(self.quote_mint.clone()));
        }
        if let Some(sig) = &self.signature {
            let ok = !sig.is_empty() && bs58_len(sig) == Some(SIGNATURE_LEN);
            if !ok {
                return Err(EventDefect::InvalidSignature(sig.clone()));
            }
        }
        if self.slot == Some(0) {
            return Err(EventDefect::ZeroSlot);
        }
        if let Some(p) = self.initial_price_sol {
            if !p.is_finite() || p < 0.0 {
                return Err(EventDefect::NonFinitePrice);
            }
        }
        if !self.launch.market_cap_sol.is_finite() || !self.launch.initial_buy_sol.is_finite() {
            return Err(EventDefect::NonFiniteLiquidity);
        }
        let skew = Duration::seconds(MAX_CLOCK_SKEW_SECS);
        if self.observed_at > now + skew {
            return Err(EventDefect::ObservedInFuture);
        }
        if let Some(ts) = self.event_ts {
            if ts > self.observed_at + skew {
                return Err(EventDefect::EventAfterObservation);
            }
        }
        if self.raw_hash.trim().is_empty() {
            return Err(EventDefect::EmptyRawHash);
        }
        if self.event_id != self.compute_event_id() {
            return Err(EventDefect::EventIdMismatch);
        }
        Ok(())
    }

    /// Slot/signature consistency between two observations of what claims
    /// to be the same event (a second feed, or a replayed fixture): equal
    /// ids must not disagree on the on-chain reference they both know.
    pub fn consistent_with(&self, other: &LaunchEvent) -> bool {
        if self.event_id != other.event_id {
            return true;
        }
        let sig_ok = match (&self.signature, &other.signature) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        let slot_ok = match (self.slot, other.slot) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        sig_ok && slot_ok && self.mint == other.mint && self.protocol == other.protocol
    }
}

/// Decoded byte length of a base58 string, `None` when it is not base58.
fn bs58_len(s: &str) -> Option<usize> {
    bs58::decode(s).into_vec().ok().map(|v| v.len())
}

/// Tracks per-source ordering so a reconnect that replays or reorders
/// notifications is observable (TASK 2 §C "reconnect / gap / ordering").
#[derive(Debug, Default)]
pub struct SequenceTracker {
    next_seq: u64,
    highest_slot: Option<u64>,
    out_of_order: u64,
}

impl SequenceTracker {
    /// Allocate the next sequence number (1-based, monotonic).
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Note the slot of a notification. Returns `true` when it regressed
    /// (an older slot after a newer one — reorder or replay).
    pub fn note_slot(&mut self, slot: Option<u64>) -> bool {
        let Some(slot) = slot.filter(|s| *s > 0) else {
            return false;
        };
        match self.highest_slot {
            Some(h) if slot < h => {
                self.out_of_order += 1;
                true
            }
            Some(h) if slot >= h => {
                self.highest_slot = Some(slot);
                false
            }
            _ => {
                self.highest_slot = Some(slot);
                false
            }
        }
    }

    pub fn highest_slot(&self) -> Option<u64> {
        self.highest_slot
    }

    pub fn out_of_order(&self) -> u64 {
        self.out_of_order
    }

    pub fn issued(&self) -> u64 {
        self.next_seq
    }
}

/// Hex digest of a raw payload for [`LaunchEvent::raw_hash`].
pub fn raw_hash_of(bytes: &[u8]) -> String {
    bot_core::execution::digest_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(fill: char) -> String {
        // 63 '1's (zero bytes) + one non-zero char = exactly 64 bytes.
        let mut s = "1".repeat(63);
        s.push(fill);
        s
    }

    fn launch(mint: &str) -> TokenLaunch {
        TokenLaunch {
            mint: mint.to_string(),
            name: "Cat".into(),
            symbol: "CAT".into(),
            uri: None,
            creator: Pubkey::new_unique().to_string(),
            pool: "bonding-curve".into(),
            initial_buy_sol: 0.5,
            market_cap_sol: 30.0,
            market_cap_usd: None,
            total_supply: None,
            slot: Some(100),
            signature: Some(sig('2')),
            tx_type: Some("create".into()),
            observed_at: Utc::now(),
            feed: LaunchFeed::PumpPortal,
            socials: None,
        }
    }

    fn event() -> LaunchEvent {
        LaunchEvent::from_token_launch(launch(&Pubkey::new_unique().to_string()), 1, "abcd".into())
    }

    #[test]
    fn event_id_is_deterministic_and_reference_aware() {
        let e = event();
        assert!(e.event_id.starts_with("evt_"));
        assert_eq!(e.event_id.len(), 4 + 32);
        assert_eq!(e.event_id, e.compute_event_id());

        // Same launch from another feed with the same signature → same id.
        let mut other = e.launch.clone();
        other.feed = LaunchFeed::SolanaLogs;
        other.observed_at = Utc::now();
        let o = LaunchEvent::from_token_launch(other, 99, "zzzz".into());
        assert_eq!(o.event_id, e.event_id);
        assert!(o.consistent_with(&e));

        // Different signature → different id.
        let mut third = e.launch.clone();
        third.signature = Some(sig('3'));
        let t = LaunchEvent::from_token_launch(third, 1, "abcd".into());
        assert_ne!(t.event_id, e.event_id);

        // No signature: falls back to the slot, then to the sequence.
        let mut no_sig = e.launch.clone();
        no_sig.signature = None;
        let a = LaunchEvent::from_token_launch(no_sig.clone(), 1, "h".into());
        let b = LaunchEvent::from_token_launch(no_sig.clone(), 2, "h".into());
        assert_eq!(
            a.event_id, b.event_id,
            "slot reference ignores the sequence"
        );
        no_sig.slot = None;
        let c = LaunchEvent::from_token_launch(no_sig.clone(), 1, "h".into());
        let d = LaunchEvent::from_token_launch(no_sig, 2, "h".into());
        assert_ne!(
            c.event_id, d.event_id,
            "sequence reference distinguishes them"
        );
        assert_ne!(a.event_id, c.event_id, "reference kind is part of the id");
    }

    #[test]
    fn dedup_key_keeps_the_legacy_mint_key_for_pump_fun() {
        let mut e = event();
        assert_eq!(e.dedup_key(), e.mint);
        e.protocol = LaunchProtocol::PumpSwap;
        assert_eq!(e.dedup_key(), format!("pump_swap:{}", e.mint));
        e.protocol = LaunchProtocol::RaydiumAmmV4;
        assert_eq!(e.dedup_key(), format!("raydium_amm_v4:{}", e.mint));
    }

    #[test]
    fn well_formed_event_validates() {
        let e = event();
        assert_eq!(e.validate_shape(Utc::now()), Ok(()));
        assert_eq!(e.base_decimals, Some(6));
        assert!(e.pool.is_some(), "curve PDA derived from the mint");
        assert_eq!(e.quote_mint, solana_kit::consts::WSOL_MINT.to_string());
    }

    #[test]
    fn malformed_events_are_rejected_with_a_specific_defect() {
        let now = Utc::now();
        let base = event();

        let mut e = base.clone();
        e.mint = "not-a-mint".into();
        e.base_mint = e.mint.clone();
        e.event_id = e.compute_event_id();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidMint(_))
        ));

        let mut e = base.clone();
        e.creator = "xyz".into();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidCreator(_))
        ));

        let mut e = base.clone();
        e.pool = Some("bad".into());
        e.event_id = e.compute_event_id();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidPool(_))
        ));

        let mut e = base.clone();
        e.protocol = LaunchProtocol::PumpSwap;
        e.pool = None;
        e.event_id = e.compute_event_id();
        assert_eq!(e.validate_shape(now), Err(EventDefect::MissingPool));

        let mut e = base.clone();
        e.base_mint = Pubkey::new_unique().to_string();
        assert_eq!(e.validate_shape(now), Err(EventDefect::BaseMintMismatch));

        let mut e = base.clone();
        e.quote_mint = "".into();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidQuoteMint(_))
        ));

        let mut e = base.clone();
        e.signature = Some("tooshort".into());
        e.event_id = e.compute_event_id();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidSignature(_))
        ));

        let mut e = base.clone();
        e.signature = Some("0OIl".into()); // non-base58 alphabet
        e.event_id = e.compute_event_id();
        assert!(matches!(
            e.validate_shape(now),
            Err(EventDefect::InvalidSignature(_))
        ));

        let mut e = base.clone();
        e.slot = Some(0);
        assert_eq!(e.validate_shape(now), Err(EventDefect::ZeroSlot));

        let mut e = base.clone();
        e.initial_price_sol = Some(f64::NAN);
        assert_eq!(e.validate_shape(now), Err(EventDefect::NonFinitePrice));

        let mut e = base.clone();
        e.launch.market_cap_sol = f64::INFINITY;
        assert_eq!(e.validate_shape(now), Err(EventDefect::NonFiniteLiquidity));

        let mut e = base.clone();
        e.observed_at = now + Duration::seconds(60);
        assert_eq!(e.validate_shape(now), Err(EventDefect::ObservedInFuture));

        let mut e = base.clone();
        e.event_ts = Some(e.observed_at + Duration::seconds(60));
        assert_eq!(
            e.validate_shape(now),
            Err(EventDefect::EventAfterObservation)
        );

        let mut e = base.clone();
        e.raw_hash = "  ".into();
        assert_eq!(e.validate_shape(now), Err(EventDefect::EmptyRawHash));

        let mut e = base.clone();
        e.event_id = "evt_forged".into();
        assert_eq!(e.validate_shape(now), Err(EventDefect::EventIdMismatch));

        let mut e = base;
        e.event_id = String::new();
        assert_eq!(e.validate_shape(now), Err(EventDefect::EmptyEventId));
    }

    #[test]
    fn staleness_uses_the_best_known_timestamp() {
        let now = Utc::now();
        let mut e = event();
        e.observed_at = now - Duration::seconds(2);
        assert!(!e.is_stale(now, 30));
        assert!(e.is_stale(now, 1));
        assert!(!e.is_stale(now, 0), "0 disables the rule");
        // A chain timestamp older than the observation makes it staler.
        e.event_ts = Some(now - Duration::seconds(45));
        assert!(e.is_stale(now, 30));
        assert!(e.age_ms(now) >= 45_000);
        assert!(e.held_ms(now) < 45_000);
        // Never negative on clock skew.
        e.event_ts = Some(now + Duration::seconds(3));
        assert_eq!(e.age_ms(now), 0);
    }

    #[test]
    fn consistency_detects_disagreeing_observations() {
        let a = event();
        let mut b = a.clone();
        b.slot = Some(101);
        assert!(!a.consistent_with(&b), "same id, different slot");
        let mut c = a.clone();
        c.slot = None;
        assert!(a.consistent_with(&c), "unknown slot is not a disagreement");
        let mut d = a.clone();
        d.event_id = "evt_other".into();
        d.slot = Some(5);
        assert!(a.consistent_with(&d), "different events never conflict");
    }

    #[test]
    fn serde_round_trip_and_forward_compat() {
        let e = event();
        let json = serde_json::to_string(&e).unwrap();
        let back: LaunchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_id, e.event_id);
        assert_eq!(back.protocol, e.protocol);
        assert_eq!(back.slot, e.slot);
        assert_eq!(back.validate_shape(Utc::now()), Ok(()));

        // Optional fields may be absent (older fixtures) and unknown fields
        // are ignored (newer producers).
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_object_mut().unwrap();
        for k in [
            "slot",
            "signature",
            "pool",
            "liquidity_quote_lamports",
            "liquidity_base_raw",
            "initial_price_sol",
            "base_decimals",
            "event_ts",
            "source_ts",
            "source_seq",
            "creator",
        ] {
            obj.remove(k);
        }
        obj.insert("future_field".into(), serde_json::json!({"x": 1}));
        let minimal: LaunchEvent = serde_json::from_value(v).expect("minimal event parses");
        assert_eq!(minimal.slot, None);
        assert_eq!(minimal.source_seq, 0);
        assert!(minimal.creator.is_empty());
        // Its id no longer matches (identity fields were stripped) — and the
        // validator says so rather than trusting the label.
        assert_eq!(
            minimal.validate_shape(Utc::now()),
            Err(EventDefect::EventIdMismatch)
        );
    }

    #[test]
    fn sequence_tracker_flags_slot_regressions() {
        let mut t = SequenceTracker::default();
        assert_eq!(t.next_seq(), 1);
        assert_eq!(t.next_seq(), 2);
        assert!(!t.note_slot(Some(10)));
        assert!(!t.note_slot(Some(10)), "equal slots are fine (same block)");
        assert!(!t.note_slot(Some(12)));
        assert!(t.note_slot(Some(11)), "regression after a reconnect");
        assert!(!t.note_slot(None), "unknown slots are ignored");
        assert!(!t.note_slot(Some(0)), "zero means unknown");
        assert_eq!(t.highest_slot(), Some(12));
        assert_eq!(t.out_of_order(), 1);
        assert_eq!(t.issued(), 2);
    }

    #[test]
    fn protocol_metadata_is_consistent() {
        assert_eq!(LaunchProtocol::PumpFun.venue(), Venue::PumpFun);
        assert_eq!(LaunchProtocol::PumpSwap.venue(), Venue::PumpSwap);
        assert_eq!(LaunchProtocol::RaydiumAmmV4.venue(), Venue::RaydiumAmmV4);
        assert!(!LaunchProtocol::PumpFun.requires_pool());
        assert!(LaunchProtocol::PumpSwap.requires_pool());
        assert_eq!(LaunchProtocol::RaydiumAmmV4.to_string(), "raydium_amm_v4");
        assert_eq!(raw_hash_of(b"x").len(), 64);
    }
}
