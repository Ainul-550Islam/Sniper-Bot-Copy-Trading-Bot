//! Async order-commit model (PROMPT 4/10 §D).
//!
//! The 2026 CLOB no longer settles synchronously with acceptance: a
//! `POST /order` response can come back **matched with trade ids but
//! no settlement hashes yet**, or **`delayed`** (accepted, queued for
//! matching, zero amounts, no ids at all). Acceptance and settlement
//! are separate events and the gap between them is normal.
//!
//! This module turns the raw [`PostOrderResponse`] into an explicit
//! [`CommitState`] with the venue's documented semantics:
//!
//! | wire status | meaning | state |
//! |---|---|---|
//! | `matched` + `transactionsHashes` non-empty | matched and settlement hashes already known | [`Settled`](CommitState::Settled) |
//! | `matched` + hashes missing/empty | matched, hashes resolve later by polling | [`MatchedAwaitingSettlement`](CommitState::MatchedAwaitingSettlement) |
//! | `live` | resting on the book, nothing filled | [`Resting`](CommitState::Resting) |
//! | `delayed` | accepted, matching deferred (market has `secondsDelay`); amounts are `"0"`, ids empty | [`PendingMatch`](CommitState::PendingMatch) |
//! | `unmatched` / `success=false` | not placed | [`Rejected`](CommitState::Rejected) |
//!
//! Invariants this module enforces:
//!
//! * a `delayed` acceptance is NEVER a fill — its zero amounts and
//!   empty `tradeIDs`/`transactionsHashes` are preserved as "nothing
//!   happened yet", not coerced into zeros that look booked;
//! * trade ids and hashes are only ever carried, never synthesized —
//!   absent stays absent until the venue says otherwise;
//! * `success=false` with an `errorMsg` is a rejection even if the
//!   status string looks otherwise.
//!
//! Source: `PROMPT-4-POLYMARKET-RESEARCH.md` §4 (official docs:
//! "Place Orders", "Post multiple orders", `wait_for_order_fill_settlement`
//! semantics in clob-client-v2 / py-clob-client-v2).

use crate::clob::PostOrderResponse;

/// The wire status of an acceptance, as the venue spells it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AcceptanceStatus {
    /// Resting on the book (`live`).
    Live,
    /// Filled fully or partially (`matched`).
    Matched,
    /// Accepted but matching is deferred (`delayed`) — pending, NOT a
    /// fill; amounts are `"0"` and id collections are empty.
    Delayed,
    /// Marketable but placement failed (`unmatched`).
    Unmatched,
    /// Anything else the venue returns (forward-compat: kept verbatim).
    Other(String),
}

impl AcceptanceStatus {
    /// Parse the venue's free-form status string.
    pub fn parse(raw: Option<&str>) -> AcceptanceStatus {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) if s.eq_ignore_ascii_case("live") => AcceptanceStatus::Live,
            Some(s) if s.eq_ignore_ascii_case("matched") => AcceptanceStatus::Matched,
            Some(s) if s.eq_ignore_ascii_case("delayed") => AcceptanceStatus::Delayed,
            Some(s) if s.eq_ignore_ascii_case("unmatched") => AcceptanceStatus::Unmatched,
            Some(s) => AcceptanceStatus::Other(s.to_string()),
            None => AcceptanceStatus::Other(String::new()),
        }
    }

    /// Stable machine-readable label.
    pub fn as_str(&self) -> &str {
        match self {
            AcceptanceStatus::Live => "live",
            AcceptanceStatus::Matched => "matched",
            AcceptanceStatus::Delayed => "delayed",
            AcceptanceStatus::Unmatched => "unmatched",
            AcceptanceStatus::Other(s) => s,
        }
    }
}

/// What we KNOW about an accepted order at acceptance time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommitState {
    /// Resting on the book (`live`): no fills, nothing pending except
    /// the resting order itself.
    Resting,
    /// Matched, trade ids known, settlement hashes not yet returned —
    /// poll the trades endpoint until terminal (see
    /// `trade_resolution`).
    MatchedAwaitingSettlement,
    /// Matched AND settlement hashes already known at acceptance time
    /// (the fast path — no backfill needed).
    Settled,
    /// Accepted but matching deferred (`delayed`): the order is
    /// PENDING, not filled. Zero amounts and empty ids are the venue's
    /// own statement that no fills exist yet.
    PendingMatch,
    /// Not placed (venue refused: `unmatched` or `success=false`).
    Rejected,
}

impl CommitState {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &str {
        match self {
            CommitState::Resting => "resting",
            CommitState::MatchedAwaitingSettlement => "matched_awaiting_settlement",
            CommitState::Settled => "settled",
            CommitState::PendingMatch => "pending_match",
            CommitState::Rejected => "rejected",
        }
    }

    /// Whether fills were reported at acceptance time (matched — even
    /// if hashes are still pending).
    pub fn has_fills(self) -> bool {
        matches!(
            self,
            CommitState::Settled | CommitState::MatchedAwaitingSettlement
        )
    }

    /// Whether more venue information is needed to call this order
    /// done (settlement hashes pending, or the match itself pending).
    pub fn requires_backfill(self) -> bool {
        matches!(
            self,
            CommitState::MatchedAwaitingSettlement | CommitState::PendingMatch
        )
    }
}

/// One parsed `POST /order` acceptance.
#[derive(Debug, Clone, PartialEq)]
pub struct AsyncOrderAcceptance {
    /// The venue order id (`orderID`), when accepted.
    pub order_id: Option<String>,
    /// The venue status.
    pub status: AcceptanceStatus,
    /// Whether the venue reports success.
    pub success: Option<bool>,
    /// Rejection reason when not accepted.
    pub error_msg: Option<String>,
    /// Filled making amount, raw 6-dec units (parsed from the decimal
    /// string; `None` when the venue omitted it).
    pub making_amount: Option<u128>,
    /// Filled taking amount, raw 6-dec units.
    pub taking_amount: Option<u128>,
    /// Trade ids for the fills (empty unless matched).
    pub trade_ids: Vec<String>,
    /// Settlement transaction hashes (may be empty even when matched —
    /// that is the async gap this module exists for).
    pub transactions_hashes: Vec<String>,
}

impl AsyncOrderAcceptance {
    /// Parse a raw [`PostOrderResponse`].
    pub fn from_response(resp: &PostOrderResponse) -> AsyncOrderAcceptance {
        AsyncOrderAcceptance {
            order_id: resp
                .order_id
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            status: AcceptanceStatus::parse(resp.status.as_deref()),
            success: resp.success,
            error_msg: resp
                .error_msg
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            making_amount: resp.making_amount.as_deref().and_then(parse_raw),
            taking_amount: resp.taking_amount.as_deref().and_then(parse_raw),
            trade_ids: resp
                .trade_ids
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            transactions_hashes: resp
                .transactions_hashes
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }

    /// Whether the venue accepted the order at all. `success=false` is
    /// a rejection regardless of the status string; an explicit
    /// `unmatched` is a rejection regardless of `success`.
    pub fn is_accepted(&self) -> bool {
        if self.success == Some(false) {
            return false;
        }
        if self.status == AcceptanceStatus::Unmatched {
            return false;
        }
        // Accepted forms: matched / live / delayed, with success either
        // true or absent (the venue does not always send it).
        matches!(
            self.status,
            AcceptanceStatus::Live
                | AcceptanceStatus::Matched
                | AcceptanceStatus::Delayed
                | AcceptanceStatus::Other(_)
        ) && self.success != Some(false)
    }

    /// Classify into a [`CommitState`] using the venue's documented
    /// semantics (see the module table).
    pub fn commit_state(&self) -> CommitState {
        if !self.is_accepted() {
            return CommitState::Rejected;
        }
        match self.status {
            AcceptanceStatus::Live => CommitState::Resting,
            AcceptanceStatus::Delayed => CommitState::PendingMatch,
            AcceptanceStatus::Matched => {
                if self.transactions_hashes.is_empty() {
                    CommitState::MatchedAwaitingSettlement
                } else {
                    CommitState::Settled
                }
            }
            // Unknown statuses stay honest: without a recognized
            // status we cannot claim fills, settlement or a resting
            // order — treat as pending-match (the most conservative
            // non-rejection: it forces a venue re-read and never books
            // a fill).
            AcceptanceStatus::Unmatched => CommitState::Rejected,
            AcceptanceStatus::Other(_) => CommitState::PendingMatch,
        }
    }

    /// Whether this acceptance reported fills (matched). A delayed
    /// acceptance never did.
    pub fn reports_fills(&self) -> bool {
        self.commit_state().has_fills()
    }

    /// Whether settlement information is still missing and must be
    /// resolved by polling (see `trade_resolution` / `backfill`).
    pub fn requires_backfill(&self) -> bool {
        self.commit_state().requires_backfill()
    }
}

/// Parse a raw 6-dec decimal string ("1000000") into u128 units;
/// `None` when absent, empty or non-numeric — never a fabricated 0.
fn parse_raw(raw: &str) -> Option<u128> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<u128>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: Option<&str>, success: Option<bool>) -> PostOrderResponse {
        PostOrderResponse {
            order_id: Some("0xabc".into()),
            success,
            error_msg: None,
            status: status.map(str::to_string),
            taking_amount: None,
            making_amount: None,
            trade_ids: None,
            transactions_hashes: None,
        }
    }

    #[test]
    fn matched_with_hashes_is_settled() {
        let mut r = resp(Some("matched"), Some(true));
        r.trade_ids = Some(vec!["trade-123".into()]);
        r.transactions_hashes = Some(vec!["0xhash1".into()]);
        r.making_amount = Some("1000000".into());
        let a = AsyncOrderAcceptance::from_response(&r);
        assert_eq!(a.commit_state(), CommitState::Settled);
        assert!(a.reports_fills());
        assert!(!a.requires_backfill());
        assert_eq!(a.making_amount, Some(1_000_000));
        assert_eq!(a.trade_ids, vec!["trade-123".to_string()]);
    }

    #[test]
    fn matched_without_hashes_awaits_settlement() {
        // the async gap: fills known, hashes not — poll for them.
        let mut r = resp(Some("matched"), Some(true));
        r.trade_ids = Some(vec!["trade-1".into(), "trade-2".into()]);
        r.transactions_hashes = None;
        let a = AsyncOrderAcceptance::from_response(&r);
        assert_eq!(a.commit_state(), CommitState::MatchedAwaitingSettlement);
        assert!(a.reports_fills());
        assert!(a.requires_backfill());
    }

    #[test]
    fn delayed_is_pending_not_a_fill() {
        // Official semantics: delayed ⇒ amounts "0", ids empty, NO
        // fills exist yet.
        let mut r = resp(Some("delayed"), Some(true));
        r.making_amount = Some("0".into());
        r.taking_amount = Some("0".into());
        r.trade_ids = Some(vec![]);
        r.transactions_hashes = Some(vec![]);
        let a = AsyncOrderAcceptance::from_response(&r);
        assert_eq!(a.commit_state(), CommitState::PendingMatch);
        assert!(!a.reports_fills());
        assert!(a.requires_backfill());
        assert_eq!(a.making_amount, Some(0));
        assert!(a.trade_ids.is_empty());
        assert!(a.transactions_hashes.is_empty());
    }

    #[test]
    fn live_rests() {
        let a = AsyncOrderAcceptance::from_response(&resp(Some("live"), Some(true)));
        assert_eq!(a.commit_state(), CommitState::Resting);
        assert!(!a.reports_fills());
        assert!(!a.requires_backfill());
    }

    #[test]
    fn unmatched_and_success_false_are_rejections() {
        let a = AsyncOrderAcceptance::from_response(&resp(Some("unmatched"), Some(true)));
        assert_eq!(a.commit_state(), CommitState::Rejected);
        assert!(!a.is_accepted());

        let mut r = resp(Some("matched"), Some(false));
        r.error_msg = Some("Rate limit exceeded".into());
        let a = AsyncOrderAcceptance::from_response(&r);
        assert_eq!(a.commit_state(), CommitState::Rejected);
        assert!(!a.is_accepted());
        // success=false wins even over a fill-looking status.
        assert!(!a.reports_fills());
    }

    #[test]
    fn unknown_status_is_conservatively_pending() {
        let a = AsyncOrderAcceptance::from_response(&resp(Some("weird-new-status"), None));
        assert_eq!(a.commit_state(), CommitState::PendingMatch);
        assert!(!a.reports_fills());
        assert!(a.requires_backfill());
    }

    #[test]
    fn statuses_parse_case_insensitively() {
        assert_eq!(
            AcceptanceStatus::parse(Some("MATCHED")),
            AcceptanceStatus::Matched
        );
        assert_eq!(
            AcceptanceStatus::parse(Some(" Live ")),
            AcceptanceStatus::Live
        );
        assert_eq!(
            AcceptanceStatus::parse(None),
            AcceptanceStatus::Other(String::new())
        );
    }

    #[test]
    fn wire_aliases_both_spellings_parse() {
        // camelCase wire forms (tradeIDs/transactionsHashes) and the
        // snake_case forms must both deserialize into the response.
        let v = serde_json::json!({
            "success": true,
            "orderID": "0xabc",
            "status": "matched",
            "makingAmount": "1000000",
            "takingAmount": "2000000",
            "tradeIDs": ["t1"],
            "transactionsHashes": ["0xh"]
        });
        let r: PostOrderResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.trade_ids.as_deref(), Some(&["t1".to_string()][..]));
        assert_eq!(
            r.transactions_hashes.as_deref(),
            Some(&["0xh".to_string()][..])
        );
        let a = AsyncOrderAcceptance::from_response(&r);
        assert_eq!(a.commit_state(), CommitState::Settled);
    }
}
