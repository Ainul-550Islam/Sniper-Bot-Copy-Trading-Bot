//! The **only** owner of audit events and of redaction.
//!
//! Every state transition of a trade command — and every consent lifecycle
//! change — writes exactly one row to `telegram_trade_audit` (migration 0067).
//! Rows are append-only and never updated.
//!
//! ## What may never appear in an audit row
//!
//! * the confirmation nonce (only its SHA-256 digest is stored, and even that
//!   lives on the command row, not here);
//! * the bot token, any signing key, any secret;
//! * free-form text straight from a chat.
//!
//! Two mechanisms enforce that: [`AuditDetail`] is a **whitelist builder** (it
//! has no method that accepts arbitrary keys, so a new field cannot be added by
//! accident), and [`scrub`] is applied to the only free-form input the module
//! ever has (a port failure message) before it is stored.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::consent::store::StoreError;
use crate::trade_port::TradeAction;

/// The closed set of audited events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEvent {
    /// A `/buy` or `/sell` request was accepted and a nonce issued.
    Requested,
    /// A request (or a confirmation) was refused. `detail.reason` says why.
    Denied,
    /// A nonce was accepted and the command moved to `confirmed`.
    Confirmed,
    /// The port accepted the order.
    Executed,
    /// The port refused or failed the order.
    Failed,
    /// The TTL elapsed before confirmation.
    Expired,
    /// The requester cancelled, or a consent revocation cancelled it.
    Cancelled,
    /// A consent was revoked.
    Revoked,
    /// A consent was granted.
    ConsentGranted,
}

impl AuditEvent {
    /// Stable label stored in `telegram_trade_audit.event`.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditEvent::Requested => "requested",
            AuditEvent::Denied => "denied",
            AuditEvent::Confirmed => "confirmed",
            AuditEvent::Executed => "executed",
            AuditEvent::Failed => "failed",
            AuditEvent::Expired => "expired",
            AuditEvent::Cancelled => "cancelled",
            AuditEvent::Revoked => "revoked",
            AuditEvent::ConsentGranted => "consent_granted",
        }
    }

    /// Parse a persisted label (unknown labels are `None`).
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "requested" => Some(AuditEvent::Requested),
            "denied" => Some(AuditEvent::Denied),
            "confirmed" => Some(AuditEvent::Confirmed),
            "executed" => Some(AuditEvent::Executed),
            "failed" => Some(AuditEvent::Failed),
            "expired" => Some(AuditEvent::Expired),
            "cancelled" => Some(AuditEvent::Cancelled),
            "revoked" => Some(AuditEvent::Revoked),
            "consent_granted" => Some(AuditEvent::ConsentGranted),
            _ => None,
        }
    }
}

/// Replace anything that looks like a secret with `<redacted>`.
///
/// Rules (deliberately blunt — a false positive costs nothing, a leak is
/// catastrophic):
/// * control characters are dropped and the text is truncated to 200 chars;
/// * a whitespace-delimited word containing `:` with an 8+ char alphanumeric
///   segment (the Telegram bot-token shape `<digits>:<secret>`) is redacted;
/// * a word containing a 16+ character alphanumeric run (nonces, base58 keys,
///   hex digests) is redacted.
pub fn scrub(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    let mut out: Vec<&str> = Vec::new();
    for word in cleaned.split_whitespace() {
        if looks_secret(word) {
            out.push("<redacted>");
        } else {
            out.push(word);
        }
    }
    out.join(" ")
}

fn looks_secret(word: &str) -> bool {
    let token_shaped = word.contains(':')
        && word.split(':').any(|segment| {
            segment.len() >= 8 && segment.chars().all(|c| c.is_ascii_alphanumeric())
        });
    token_shaped || longest_alnum_run(word) >= 16
}

fn longest_alnum_run(word: &str) -> usize {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in word.chars() {
        if ch.is_ascii_alphanumeric() {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// A whitelisted audit detail. Only the methods below exist, so nothing else
/// can end up in the JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditDetail(Map<String, Value>);

impl AuditDetail {
    /// An empty detail.
    pub fn new() -> Self {
        Self::default()
    }

    /// The machine reason code of a denial or failure (scrubbed).
    pub fn reason(mut self, code: &str) -> Self {
        self.0.insert("reason".into(), Value::String(scrub(code)));
        self
    }

    /// The action the command targets.
    pub fn action(mut self, action: TradeAction) -> Self {
        self.0
            .insert("action".into(), Value::String(action.as_str().into()));
        self
    }

    /// A lamport amount (buy size, cap, spend…).
    pub fn lamports(mut self, key: AuditAmount, lamports: u64) -> Self {
        self.0.insert(key.as_str().into(), Value::from(lamports));
        self
    }

    /// A basis-point value (sell fraction or slippage).
    pub fn bps(mut self, key: AuditBps, bps: u16) -> Self {
        self.0.insert(key.as_str().into(), Value::from(bps));
        self
    }

    /// The execution mode the command was accepted under.
    pub fn mode(mut self, mode: &str) -> Self {
        self.0.insert("mode".into(), Value::String(scrub(mode)));
        self
    }

    /// The status a command moved to.
    pub fn status(mut self, status: &str) -> Self {
        self.0.insert("status".into(), Value::String(scrub(status)));
        self
    }

    /// The consent the decision was based on.
    pub fn consent_id(mut self, consent_id: Uuid) -> Self {
        self.0
            .insert("consent_id".into(), Value::String(consent_id.to_string()));
        self
    }

    /// Number of affected rows (e.g. cancelled pending commands).
    pub fn affected(mut self, count: u64) -> Self {
        self.0.insert("affected".into(), Value::from(count));
        self
    }

    /// The token mint.
    ///
    /// Recorded **verbatim**: a mint is public chain data (an operator
    /// debugging an order needs to know which token it targeted) and every mint
    /// reaching this builder has already been canonicalised by
    /// [`crate::commands::parse::parse_mint`] (base58, exactly 32 bytes) or read
    /// back from a stored command row. Scrubbing it would redact every mint,
    /// because mints are 32+ character alphanumeric runs.
    pub fn mint(mut self, mint: &str) -> Self {
        self.0
            .insert("mint".into(), Value::String(mint.to_string()));
        self
    }

    /// A free-form failure message from the executing side — always scrubbed.
    pub fn failure_detail(mut self, detail: &str) -> Self {
        self.0
            .insert("failure_detail".into(), Value::String(scrub(detail)));
        self
    }

    /// Borrow the underlying map.
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// Rebuild a detail object from a stored JSON map, **dropping** every key
    /// that is not on [`AuditDetailKeys::all`].
    ///
    /// Reading is where a compromised or hand-edited row would try to smuggle a
    /// field into `Debug`/API output, so the whitelist is applied on the way in
    /// as well as on the way out.
    pub fn from_map(map: Map<String, Value>) -> Self {
        let allowed = AuditDetailKeys::all();
        let mut clean = Map::new();
        for (key, value) in map {
            if allowed.contains(&key.as_str()) {
                clean.insert(key, value);
            }
        }
        Self(clean)
    }
}

/// The **complete** set of keys [`AuditDetail`] can emit.
///
/// Kept in one place so a redaction test (and a reviewer) can assert the whole
/// surface: anything not in this list can never be written to an audit row.
pub struct AuditDetailKeys;

impl AuditDetailKeys {
    /// Every key, in a stable order.
    pub fn all() -> Vec<&'static str> {
        let mut keys = vec![
            "reason",
            "action",
            "mode",
            "status",
            "consent_id",
            "affected",
            "mint",
            "failure_detail",
        ];
        keys.extend([
            AuditAmount::AmountLamports.as_str(),
            AuditAmount::OrderCapLamports.as_str(),
            AuditAmount::DailyCapLamports.as_str(),
            AuditAmount::SpentLamports.as_str(),
            AuditBps::PercentBps.as_str(),
            AuditBps::SlippageBps.as_str(),
        ]);
        keys
    }
}

/// Whitelisted lamport-valued detail keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAmount {
    /// The requested per-order amount.
    AmountLamports,
    /// The per-order cap in force.
    OrderCapLamports,
    /// The rolling 24h cap in force.
    DailyCapLamports,
    /// Already committed in the rolling window.
    SpentLamports,
}

impl AuditAmount {
    /// The JSON key.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditAmount::AmountLamports => "amount_lamports",
            AuditAmount::OrderCapLamports => "order_cap_lamports",
            AuditAmount::DailyCapLamports => "daily_cap_lamports",
            AuditAmount::SpentLamports => "spent_lamports",
        }
    }
}

/// Whitelisted basis-point detail keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditBps {
    /// The sell fraction.
    PercentBps,
    /// The slippage tolerance.
    SlippageBps,
}

impl AuditBps {
    /// The JSON key.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditBps::PercentBps => "percent_bps",
            AuditBps::SlippageBps => "slippage_bps",
        }
    }
}

/// One row of `telegram_trade_audit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeAuditEvent {
    /// `bigserial` primary key; `None` before the store assigns it.
    pub audit_id: Option<i64>,
    /// The command this event belongs to, when there is one.
    pub command_id: Option<Uuid>,
    /// Tenant that owns the chat binding.
    pub organization_id: String,
    /// Telegram chat the event happened in.
    pub chat_id: i64,
    /// What happened.
    pub event: AuditEvent,
    /// Whitelisted detail.
    pub detail: AuditDetail,
    /// When it happened.
    pub created_at: DateTime<Utc>,
}

impl TradeAuditEvent {
    /// Build a new event (audit id assigned by the store).
    pub fn new(
        command_id: Option<Uuid>,
        organization_id: impl Into<String>,
        chat_id: i64,
        event: AuditEvent,
        detail: AuditDetail,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            audit_id: None,
            command_id,
            organization_id: organization_id.into(),
            chat_id,
            event,
            detail,
            created_at,
        }
    }
}

/// Durable audit rows.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Append one row. Audit failures are never silent: the caller decides
    /// whether the operation may continue, and the decision is always "no"
    /// for a state-changing step.
    async fn append(&self, event: &TradeAuditEvent) -> Result<(), StoreError>;

    /// The newest rows of one organization, newest first.
    async fn recent(
        &self,
        organization_id: &str,
        limit: usize,
    ) -> Result<Vec<TradeAuditEvent>, StoreError>;
}

/// A process-local [`AuditStore`].
#[derive(Debug, Default)]
pub struct InMemoryAuditStore {
    rows: Mutex<Vec<TradeAuditEvent>>,
    next_id: AtomicI64,
}

impl InMemoryAuditStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every row, oldest first (diagnostics and tests).
    pub fn all(&self) -> Vec<TradeAuditEvent> {
        self.rows.lock().expect("audit store lock").clone()
    }

    /// The whole store rendered as one JSON string — what a redaction test
    /// scans for nonce/token material.
    pub fn render(&self) -> String {
        let rows: Vec<Value> = self
            .all()
            .into_iter()
            .map(|row| {
                let mut object = Map::new();
                object.insert("event".into(), Value::String(row.event.as_str().into()));
                object.insert("chat_id".into(), Value::from(row.chat_id));
                object.insert(
                    "organization_id".into(),
                    Value::String(row.organization_id.clone()),
                );
                if let Some(id) = row.command_id {
                    object.insert("command_id".into(), Value::String(id.to_string()));
                }
                object.insert("detail".into(), Value::Object(row.detail.as_map().clone()));
                Value::Object(object)
            })
            .collect();
        Value::Array(rows).to_string()
    }
}

#[async_trait]
impl AuditStore for InMemoryAuditStore {
    async fn append(&self, event: &TradeAuditEvent) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().expect("audit store lock");
        let mut row = event.clone();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        row.audit_id = Some(id);
        rows.push(row);
        Ok(())
    }

    async fn recent(
        &self,
        organization_id: &str,
        limit: usize,
    ) -> Result<Vec<TradeAuditEvent>, StoreError> {
        let rows = self.rows.lock().expect("audit store lock");
        Ok(rows
            .iter()
            .filter(|r| r.organization_id == organization_id)
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }
}

/// An audit store that always fails — proves that a ledger outage stops
/// trading instead of letting it run unaudited.
#[derive(Debug, Default)]
pub struct BrokenAuditStore {
    /// Detail surfaced on every call.
    pub detail: String,
}

impl BrokenAuditStore {
    /// Build one with a fixed detail string.
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    fn err(&self) -> StoreError {
        StoreError::Unavailable(if self.detail.is_empty() {
            "audit store unavailable".to_string()
        } else {
            self.detail.clone()
        })
    }
}

#[async_trait]
impl AuditStore for BrokenAuditStore {
    async fn append(&self, _event: &TradeAuditEvent) -> Result<(), StoreError> {
        Err(self.err())
    }

    async fn recent(
        &self,
        _organization_id: &str,
        _limit: usize,
    ) -> Result<Vec<TradeAuditEvent>, StoreError> {
        Err(self.err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn event_labels_round_trip() {
        for event in [
            AuditEvent::Requested,
            AuditEvent::Denied,
            AuditEvent::Confirmed,
            AuditEvent::Executed,
            AuditEvent::Failed,
            AuditEvent::Expired,
            AuditEvent::Cancelled,
            AuditEvent::Revoked,
            AuditEvent::ConsentGranted,
        ] {
            assert_eq!(AuditEvent::from_label(event.as_str()), Some(event));
        }
        assert_eq!(AuditEvent::from_label("executed!"), None);
    }

    #[test]
    fn detail_builder_only_accepts_whitelisted_keys() {
        let detail = AuditDetail::new()
            .reason("no_consent")
            .action(TradeAction::Buy)
            .lamports(AuditAmount::AmountLamports, 50_000_000)
            .lamports(AuditAmount::OrderCapLamports, 40_000_000)
            .lamports(AuditAmount::DailyCapLamports, 200_000_000)
            .lamports(AuditAmount::SpentLamports, 10_000_000)
            .bps(AuditBps::PercentBps, 1_250)
            .bps(AuditBps::SlippageBps, 100)
            .mode("paper")
            .status("denied")
            .consent_id(Uuid::from_u128(9))
            .affected(3)
            .mint("So11111111111111111111111111111111111111112")
            .failure_detail("risk engine: max exposure");
        let keys: Vec<&str> = detail.as_map().keys().map(String::as_str).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec![
                "action",
                "affected",
                "amount_lamports",
                "consent_id",
                "daily_cap_lamports",
                "failure_detail",
                "mint",
                "mode",
                "order_cap_lamports",
                "percent_bps",
                "reason",
                "slippage_bps",
                "spent_lamports",
                "status",
            ]
        );
    }

    #[test]
    fn the_builder_can_only_produce_whitelisted_keys() {
        let detail = AuditDetail::new()
            .reason("x")
            .action(TradeAction::Buy)
            .lamports(AuditAmount::AmountLamports, 1)
            .lamports(AuditAmount::OrderCapLamports, 1)
            .lamports(AuditAmount::DailyCapLamports, 1)
            .lamports(AuditAmount::SpentLamports, 1)
            .bps(AuditBps::PercentBps, 1)
            .bps(AuditBps::SlippageBps, 1)
            .mode("paper")
            .status("pending")
            .consent_id(Uuid::from_u128(1))
            .affected(1)
            .mint("m")
            .failure_detail("d");
        let whitelist = AuditDetailKeys::all();
        for key in detail.as_map().keys() {
            assert!(
                whitelist.contains(&key.as_str()),
                "non-whitelisted key {key}"
            );
        }
        let mut sorted = whitelist.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), whitelist.len(), "duplicate whitelist entry");
    }

    #[test]
    fn scrub_removes_nonce_and_token_material() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let token = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
        let key = "5J8tGmYq3kR9wVbN2xPzL7cDfH4uEsA1nM6yT0iKoBvQ";
        let message = format!("nonce {nonce} bot {token} key {key} plain words stay");
        let scrubbed = scrub(&message);
        assert!(!scrubbed.contains(nonce), "{scrubbed}");
        assert!(!scrubbed.contains(token), "{scrubbed}");
        assert!(!scrubbed.contains(key), "{scrubbed}");
        assert!(scrubbed.contains("plain words stay"), "{scrubbed}");
        assert_eq!(scrub("max exposure exceeded"), "max exposure exceeded");
        assert_eq!(scrub(&"x".repeat(500)).len(), "<redacted>".len());
        assert_eq!(scrub("line\nbreak"), "line break");
        assert!(scrub("   ").is_empty());
    }

    #[test]
    fn scrub_keeps_short_alphanumeric_words() {
        // A digest fragment shorter than the 16-char threshold is not secret.
        assert_eq!(scrub("code abcdef123"), "code abcdef123");
        assert_eq!(scrub("abcdefghijklmnop"), "<redacted>");
        assert_eq!(scrub("abcdefghijklmno"), "abcdefghijklmno");
    }

    #[tokio::test]
    async fn memory_store_assigns_ids_and_filters_by_org() {
        let store = InMemoryAuditStore::new();
        let detail = AuditDetail::new().reason("no_consent");
        store
            .append(&TradeAuditEvent::new(
                None,
                "org-1",
                100,
                AuditEvent::Denied,
                detail.clone(),
                at(1),
            ))
            .await
            .unwrap();
        store
            .append(&TradeAuditEvent::new(
                Some(Uuid::from_u128(1)),
                "org-2",
                200,
                AuditEvent::Requested,
                AuditDetail::new(),
                at(2),
            ))
            .await
            .unwrap();
        let rows = store.recent("org-1", 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].audit_id, Some(1));
        assert_eq!(rows[0].event, AuditEvent::Denied);
        assert!(store.recent("org-3", 10).await.unwrap().is_empty());
        assert_eq!(store.all().len(), 2);
        let rendered = store.render();
        assert!(rendered.contains("no_consent"), "{rendered}");
        assert!(!rendered.contains("nonce"), "{rendered}");
    }

    #[tokio::test]
    async fn broken_store_always_errors() {
        let store = BrokenAuditStore::new("ledger write failed");
        assert!(store
            .append(&TradeAuditEvent::new(
                None,
                "org-1",
                1,
                AuditEvent::Denied,
                AuditDetail::new(),
                at(0)
            ))
            .await
            .is_err());
        assert!(store.recent("org-1", 1).await.is_err());
        assert_eq!(
            BrokenAuditStore::default()
                .recent("org-1", 1)
                .await
                .unwrap_err()
                .code(),
            "store_unavailable"
        );
    }
}
