//! The live probe: ask the CLOB about an ambiguous submission.
//!
//! Split from [`crate::venue_state`] (which is pure) so the decision table can
//! be tested without HTTP and exercised by a mock venue, matching the style of
//! `crates/server/src/reservation_chain.rs`.

use crate::clob::ClobClient;
use crate::error::PolyError;
use crate::venue_state::{judge, VenueStateCheck};

/// Ask the venue about one ambiguous submission.
///
/// Both reads are attempted even when the first fails, but ANY failure makes
/// the answer `Unknown` — a partial view is never read as absence.
///
/// The token-scoped open-order read is best-effort on its own: if it fails
/// while `/data/order` answered with the derived id, the landing still stands
/// (the positive answer needs no corroboration). Every other combination
/// degrades to `Unknown`.
pub async fn probe_order(
    client: &ClobClient,
    derived_order_id: &str,
    token_id: &str,
) -> VenueStateCheck {
    // A malformed id is refused before the venue is asked; `judge` encodes
    // that rule, so consult it first with a probe error of `None`.
    if !crate::venue_state::is_well_formed_order_id(derived_order_id) {
        // Nothing well-formed to ask about: refuse locally, never guess at the
        // venue's data service.
        return judge(derived_order_id, token_id, None, &[], None);
    }
    let mut probe_error: Option<PolyError> = None;
    let mut found = None;
    match client.order(derived_order_id).await {
        Ok(o) => found = o,
        Err(e) => probe_error = Some(e),
    }
    let mut open = Vec::new();
    if probe_error.is_none() && found.is_none() && !token_id.trim().is_empty() {
        match client.open_orders(None, Some(token_id)).await {
            Ok(list) => open = list,
            Err(e) => probe_error = Some(e),
        }
    }
    judge(
        derived_order_id,
        token_id,
        found.as_ref(),
        &open,
        probe_error.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOKEN: &str = "1234567890";

    #[tokio::test]
    async fn a_malformed_id_is_refused_without_any_network_call() {
        // The base URL is unroutable; if the client were consulted this would
        // block or error differently. The verdict must be the local refusal.
        let client = ClobClient::new("http://127.0.0.1:1", 137).expect("client");
        let check = probe_order(&client, "not-an-order-id", TOKEN).await;
        assert!(check.presence.is_unknown());
        assert!(check.reason.contains("not asking the venue"));
    }

    #[tokio::test]
    async fn an_unreachable_venue_is_unknown_not_absent() {
        let client = ClobClient::new("http://127.0.0.1:1", 137).expect("client");
        let check = probe_order(&client, ID, TOKEN).await;
        assert!(check.presence.is_unknown(), "got {:?}", check.presence);
        assert!(!check.link_allowed() && !check.abandon_allowed());
    }
}
