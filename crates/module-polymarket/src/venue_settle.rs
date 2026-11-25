//! Settle an ambiguous submit's durable claim against what the VENUE says.
//!
//! [`crate::venue_state`] answers what the CLOB holds for a derived order id.
//! This module is the only place allowed to act on that answer, under one rule:
//! **a claim is released only when the venue itself is the proof that nothing
//! was accepted.**
//!
//! | venue answer | claim | order state |
//! |---|---|---|
//! | `landed` | kept (settled by the normal link/fill path) | left alone |
//! | `absent`, order `submitted`/`unknown`, never acknowledged by the venue | released as an established non-send (when the sink allows it) | `Failed` — "venue never saw it" |
//! | `absent`, but the venue acknowledged it at some point | kept | unchanged, escalated to a human |
//! | `mismatch` | kept | unchanged, escalated to a human |
//! | `unknown` / probe error | kept | unchanged, retried next pass |
//!
//! Why `absent` is enough HERE but not in the operator sweep: the sweep frees a
//! trade key so a NEW broadcast may run, with nothing else corroborating the
//! non-send. This path only touches an order that is `Submitted`/`Unknown`
//! **and** was never acknowledged by the venue, and it ends that order as
//! `Failed` instead of implying a retry. Absence still never books a fill.

use crate::error::PolyError;
use crate::orders::{LocalOrderState, TrackedOrder};
use crate::venue_state::{VenuePresence, VenueStateCheck};
use crate::PolyBot;

/// What happened to one ambiguous claim.
#[derive(Debug, Clone, PartialEq)]
pub struct VenueSettlement {
    /// The venue's verdict.
    pub presence: VenuePresence,
    /// True when THIS pass released the durable claim as an established
    /// non-send (a retry for the token is possible again).
    pub claim_released: bool,
    /// True when this pass moved the order to a terminal state.
    pub order_closed: bool,
    /// The order state after this pass.
    pub state: &'static str,
    /// Operator-facing detail (the caller journals and audits it).
    pub detail: String,
}

impl VenueSettlement {
    /// Stable label for metrics/audit (`landed` … `unknown` + effect).
    pub fn action(&self) -> String {
        if self.claim_released {
            format!("claim_released_{}", self.presence.as_str())
        } else if self.order_closed {
            format!("order_closed_{}", self.presence.as_str())
        } else {
            format!("held_{}", self.presence.as_str())
        }
    }
}

impl PolyBot {
    /// The durable reservation key this bot uses for an outcome token.
    pub(crate) fn reservation_key_for_token(token_id: &str) -> String {
        format!("poly:entry:{token_id}")
    }

    /// Ask the venue about one ambiguous tracked order and settle its claim.
    ///
    /// Idempotent: a second call on a settled order changes nothing.
    pub async fn settle_claim_with_venue(
        &self,
        tracked: &mut TrackedOrder,
        check: &VenueStateCheck,
    ) -> Result<VenueSettlement, PolyError> {
        if !check
            .derived_order_id
            .eq_ignore_ascii_case(&tracked.venue_order_id)
        {
            return Err(PolyError::clob(format!(
                "venue check is for order {} but the tracked order is {}",
                check.derived_order_id, tracked.venue_order_id
            )));
        }
        let ambiguous = matches!(
            tracked.state,
            LocalOrderState::Submitted | LocalOrderState::Unknown
        );
        if !ambiguous {
            return Ok(VenueSettlement {
                presence: check.presence,
                claim_released: false,
                order_closed: false,
                state: tracked.state.as_str(),
                detail: format!(
                    "order already {} — venue answer {} is informational",
                    tracked.state.as_str(),
                    check.presence.as_str()
                ),
            });
        }

        match check.presence {
            VenuePresence::Landed => Ok(VenueSettlement {
                presence: check.presence,
                claim_released: false,
                order_closed: false,
                state: tracked.state.as_str(),
                detail: format!(
                    "venue holds the order ({}); the claim stays until the order settles",
                    check.reason
                ),
            }),
            VenuePresence::Absent => {
                if tracked.venue_acknowledged() {
                    // It was acknowledged once and has now vanished: a purge
                    // after a full match looks exactly like this. Never release.
                    return Ok(VenueSettlement {
                        presence: check.presence,
                        claim_released: false,
                        order_closed: false,
                        state: tracked.state.as_str(),
                        detail: format!(
                            "venue reports no order although it once acknowledged it ({}); \
                             held for a human",
                            check.reason
                        ),
                    });
                }
                // Never acknowledged and the venue holds nothing for it: the
                // broadcast provably never reached the book.
                let key = Self::reservation_key_for_token(&tracked.token_id);
                let released = self.release_claim(&key, true).await.unwrap_or(false);
                self.finish_locally(
                    tracked,
                    LocalOrderState::Failed,
                    "venue never saw the order",
                )
                .await;
                Ok(VenueSettlement {
                    presence: check.presence,
                    claim_released: released,
                    order_closed: true,
                    state: tracked.state.as_str(),
                    detail: format!(
                        "venue holds nothing for {} and never acknowledged it; claim {}",
                        tracked.venue_order_id,
                        if released { "released" } else { "kept" }
                    ),
                })
            }
            VenuePresence::Mismatch => Ok(VenueSettlement {
                presence: check.presence,
                claim_released: false,
                order_closed: false,
                state: tracked.state.as_str(),
                detail: format!(
                    "venue answered inconsistently ({}); human decision required",
                    check.reason
                ),
            }),
            VenuePresence::Unknown => Ok(VenueSettlement {
                presence: check.presence,
                claim_released: false,
                order_closed: false,
                state: tracked.state.as_str(),
                detail: format!(
                    "no usable venue answer ({}); retrying on the next pass",
                    check.reason
                ),
            }),
        }
    }

    /// Release the durable claim for `key` through the attached sink.
    ///
    /// `None` when no journal-backed sink is attached (paper/test doubles).
    async fn release_claim(&self, key: &str, established_non_send: bool) -> Option<bool> {
        let sink = self.intents.as_ref()?;
        sink.release_reservation(key, established_non_send).await
    }
}
