//! Venue-side existence check for an ambiguous Polymarket submission.
//!
//! The durable broadcast claim makes a duplicate POST impossible, but it says
//! nothing about what the **venue** knows. When `POST /order` times out (or the
//! process dies mid-request) the local state is `Unknown` while the CLOB may
//! hold a resting or matched order. This module asks the venue the only two
//! questions that can be answered safely, and answers them **without ever
//! guessing**:
//!
//! 1. `GET /data/order?order_id=<derived id>` — the id we can compute locally
//!    from the signed EIP-712 struct hash, i.e. **before** and **without** the
//!    POST response.
//! 2. `GET /data/orders?asset_id=…` — the open orders on the token, used to
//!    catch the case where the venue assigned a different handle than the one
//!    we derived.
//!
//! The decision table is deliberately fail-closed:
//!
//! | venue answer | verdict | what it authorises |
//! |---|---|---|
//! | order found, id matches derived id | `Landed` | link the order (never abandon) |
//! | open orders for the token, none matching | `Absent` | nothing on its own — an operator must still assert the order was never sent |
//! | order found under a different id for the same token | `Mismatch` | nothing; a human decides |
//! | 404 / empty | `Absent` | as above |
//! | transport error, unauthenticated, malformed id | `Unknown` | nothing |
//!
//! `Absent` is **not** proof of a non-send: `/data/order` 404s for an order the
//! venue never saw *and* for one it has already fully matched and purged, and
//! a read against a lagging node of the venue's own data service can miss a
//! just-accepted order. This module therefore never releases a claim; it only
//! reports.

use serde::Serialize;

use crate::clob::ClobOrder;
use crate::error::PolyError;

/// What the venue said about one ambiguous submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VenuePresence {
    /// The venue holds an order under the id we derived locally.
    Landed,
    /// The venue holds nothing we can attribute to this submission.
    Absent,
    /// The venue holds an order for this token under a DIFFERENT id than the
    /// one derived from our signed struct — reconciliation-integrity signal.
    Mismatch,
    /// No usable answer: transport failure, missing auth, malformed id.
    Unknown,
}

impl VenuePresence {
    /// True only for a definite landing.
    pub fn is_landed(self) -> bool {
        matches!(self, VenuePresence::Landed)
    }
    /// True when nothing at all may be concluded (the caller must retry or
    /// escalate to a human).
    pub fn is_unknown(self) -> bool {
        matches!(self, VenuePresence::Unknown)
    }
    /// Stable wire label.
    pub fn as_str(self) -> &'static str {
        match self {
            VenuePresence::Landed => "landed",
            VenuePresence::Absent => "absent",
            VenuePresence::Mismatch => "mismatch",
            VenuePresence::Unknown => "unknown",
        }
    }
}

/// The full answer: presence plus the venue's own report when it had one.
#[derive(Debug, Clone, Serialize)]
pub struct VenueStateCheck {
    /// The order id derived locally from the signed struct.
    pub derived_order_id: String,
    /// Outcome token the order was for.
    pub token_id: String,
    /// Presence verdict.
    pub presence: VenuePresence,
    /// Venue-reported order status (`live`/`matched`/…), when it answered.
    pub venue_status: Option<String>,
    /// Matched size the venue reported, when it answered.
    pub size_matched: Option<String>,
    /// Ids of the open orders seen on the token (bounded) — so a human can see
    /// exactly what the venue did hold when we said `Absent`/`Mismatch`.
    pub open_order_ids: Vec<String>,
    /// Why the verdict is not `Landed`, in operator language.
    pub reason: String,
}

impl VenueStateCheck {
    fn new(derived: &str, token: &str, presence: VenuePresence, reason: String) -> Self {
        Self {
            derived_order_id: derived.to_string(),
            token_id: token.to_string(),
            presence,
            venue_status: None,
            size_matched: None,
            open_order_ids: Vec::new(),
            reason,
        }
    }
    /// True only when the caller may link the claimed order to the venue's.
    pub fn link_allowed(&self) -> bool {
        self.presence.is_landed()
    }
    /// A release/abandon is NEVER authorised by this check alone.
    pub fn abandon_allowed(&self) -> bool {
        false
    }
}

/// An order id we can ask the venue about. The CLOB id is `0x` + 32 bytes of
/// hex (the EIP-712 struct hash); anything else is refused locally rather than
/// sent to the venue as a guessed query.
pub fn is_well_formed_order_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("0x") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Judge the venue's report for one derived order id.
///
/// `order` is the typed answer from `GET /data/order` (`None` = the venue
/// answered with no such order), `open_orders` is the token's open orders
/// (already narrowed by the client), and `probe_error` is a transport/auth
/// failure from either call — an error anywhere makes the whole answer
/// `Unknown`, because a partial view must not be read as absence.
pub fn judge(
    derived_order_id: &str,
    token_id: &str,
    order: Option<&ClobOrder>,
    open_orders: &[ClobOrder],
    probe_error: Option<&PolyError>,
) -> VenueStateCheck {
    if let Some(e) = probe_error {
        return VenueStateCheck::new(
            derived_order_id,
            token_id,
            VenuePresence::Unknown,
            format!("venue probe failed: {e}"),
        );
    }
    if !is_well_formed_order_id(derived_order_id) {
        return VenueStateCheck::new(
            derived_order_id,
            token_id,
            VenuePresence::Unknown,
            "derived order id is not a 0x-prefixed 32-byte hex id; not asking the venue"
                .to_string(),
        );
    }

    let open_ids: Vec<String> = open_orders
        .iter()
        .map(|o| o.id.clone())
        .filter(|id| !id.is_empty())
        .collect();

    if let Some(o) = order {
        if o.id.eq_ignore_ascii_case(derived_order_id) {
            let mut check = VenueStateCheck::new(
                derived_order_id,
                token_id,
                VenuePresence::Landed,
                format!(
                    "venue holds order {} in state {}",
                    o.id,
                    if o.status.is_empty() {
                        "<unreported>"
                    } else {
                        &o.status
                    }
                ),
            );
            check.venue_status = Some(o.status.clone());
            check.size_matched = o.size_matched_opt().map(|v| v.to_string());
            check.open_order_ids = open_ids;
            return check;
        }
        // The venue answered, but under a handle we did not derive: the signed
        // struct and the venue's id disagree — a human must look at this.
        let mut check = VenueStateCheck::new(
            derived_order_id,
            token_id,
            VenuePresence::Mismatch,
            format!(
                "venue answered with order {} (state {}), which is not the id derived from the \
                 signed struct",
                o.id, o.status
            ),
        );
        check.venue_status = Some(o.status.clone());
        check.size_matched = o.size_matched_opt().map(|v| v.to_string());
        check.open_order_ids = open_ids;
        return check;
    }

    if let Some(other) = open_orders.iter().find(|o| {
        !o.asset_id.is_empty() && o.asset_id.eq_ignore_ascii_case(token_id) && !o.id.is_empty()
    }) {
        let mut check = VenueStateCheck::new(
            derived_order_id,
            token_id,
            VenuePresence::Mismatch,
            format!(
                "no order for the derived id, but the venue holds {} on the same token",
                other.id
            ),
        );
        check.venue_status = Some(other.status.clone());
        check.open_order_ids = open_ids;
        return check;
    }

    let mut check = VenueStateCheck::new(
        derived_order_id,
        token_id,
        VenuePresence::Absent,
        "venue reports no order for the derived id and no open order on the token".to_string(),
    );
    check.open_order_ids = open_ids;
    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PolyError;

    const ID: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
    const TOKEN: &str =
        "71321045679252212594626385532706912750332728571942532289631379312455583992563";

    fn order(id: &str, status: &str, asset: &str) -> ClobOrder {
        ClobOrder {
            id: id.to_string(),
            status: status.to_string(),
            asset_id: asset.to_string(),
            size_matched: "12.5".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn venue_holding_the_derived_order_is_landed_and_linkable_but_never_abandonable() {
        let o = order(ID, "matched", TOKEN);
        let c = judge(ID, TOKEN, Some(&o), &[], None);
        assert_eq!(c.presence, VenuePresence::Landed);
        assert!(c.link_allowed());
        assert!(!c.abandon_allowed(), "a landing never authorises a release");
        assert_eq!(c.venue_status.as_deref(), Some("matched"));
        assert_eq!(c.size_matched.as_deref(), Some("12.5"));
    }

    #[test]
    fn a_404_is_absent_but_authorises_nothing() {
        let c = judge(ID, TOKEN, None, &[], None);
        assert_eq!(c.presence, VenuePresence::Absent);
        assert!(!c.link_allowed());
        assert!(!c.abandon_allowed(), "absence is not proof of a non-send");
    }

    #[test]
    fn an_open_order_under_another_id_is_a_mismatch() {
        let other = order("0x2222", "live", TOKEN);
        let c = judge(ID, TOKEN, None, std::slice::from_ref(&other), None);
        assert_eq!(c.presence, VenuePresence::Mismatch);
        assert_eq!(c.open_order_ids, vec!["0x2222".to_string()]);
        assert!(!c.link_allowed());
        assert!(!c.abandon_allowed());
    }

    #[test]
    fn an_answer_for_a_different_id_is_a_mismatch_not_a_landing() {
        let other = order("0xdeadbeef", "live", TOKEN);
        let c = judge(ID, TOKEN, Some(&other), &[], None);
        assert_eq!(c.presence, VenuePresence::Mismatch);
        assert!(!c.link_allowed());
    }

    #[test]
    fn a_transport_error_is_unknown_and_authorises_nothing() {
        let e = PolyError::http("connection reset");
        let c = judge(ID, TOKEN, None, &[], Some(&e));
        assert!(c.presence.is_unknown());
        assert!(!c.link_allowed() && !c.abandon_allowed());
        assert!(c.reason.contains("venue probe failed"));
    }

    #[test]
    fn a_malformed_id_is_never_sent_to_the_venue() {
        for bad in ["", "0x", "abcd", "0xzz", &format!("0x{}", "1".repeat(63))] {
            let c = judge(bad, TOKEN, None, &[], None);
            assert!(c.presence.is_unknown(), "{bad:?} must be unknown");
            assert!(c.reason.contains("not asking the venue"), "{bad:?}");
        }
        assert!(is_well_formed_order_id(ID));
        // Case-insensitive: the venue may preserve or lowercase the hex.
        assert!(is_well_formed_order_id(
            &ID.to_uppercase().replacen("0X", "0x", 1)
        ));
    }

    #[test]
    fn an_open_order_for_another_token_does_not_create_a_mismatch() {
        let other = order("0x2222", "live", "999");
        let c = judge(ID, TOKEN, None, std::slice::from_ref(&other), None);
        assert_eq!(c.presence, VenuePresence::Absent);
        // ...but the operator still sees what the venue did hold.
        assert_eq!(c.open_order_ids, vec!["0x2222".to_string()]);
    }
}
