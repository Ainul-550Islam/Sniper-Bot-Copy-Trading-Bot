//! Telegram trade consent (BATCH 04).
//!
//! Layering, and the single owner of each concern:
//!
//! * [`model`] — the [`Consent`](model::Consent) row, its scope and its typed
//!   errors. No rules live here.
//! * [`policy`] — the **only** place that decides whether a consent may exist
//!   or may be used (phrase, TTL bounds, caps against the organization's risk
//!   limits, active/expired/revoked, scope).
//! * [`store`] — the persistence seam ([`store::ConsentStore`]) plus the
//!   in-memory implementation. The Postgres implementation lives in the server
//!   crate (`sniper_suite::telegram_trade_store_pg`) because this crate has no
//!   database dependency.
//!
//! The user-facing handlers are in [`crate::commands::consent_cmd`].
//!
//! # Why the three files are separate
//!
//! A consent is the capability that lets a chat spend money, so the questions
//! "what is stored" ([`model`]), "may it be used" ([`policy`]) and "where does
//! it live" ([`store`]) must be answerable independently. If they were one
//! file, a change to persistence could quietly change a rule; because the rule
//! layer is a pure function of a row and a clock, it is exhaustively testable
//! and cannot be bypassed by a store implementation.
//!
//! # Invariants this module guarantees
//!
//! * No consent row can outlive its organization's risk limits: the draft is
//!   refused, never clamped ([`policy::draft_consent`]).
//! * At most one live consent exists per `(organization, chat)` — enforced by
//!   [`store::ConsentStore::insert`] *and* by the partial unique index in
//!   migration `0066`.
//! * Only the SHA-256 of the confirmation phrase is retained
//!   ([`policy::phrase_sha256`]); the phrase itself never leaves the request.

pub mod model;
pub mod policy;
pub mod store;

pub use model::{Consent, ConsentError, ConsentScope, ConsentStatus};
pub use policy::{
    draft_consent, effective_daily_cap, effective_order_cap, phrase_matches, phrase_sha256,
    require_active, ConsentBounds, ConsentRequest, GrantContext, CONSENT_PHRASE,
};
pub use store::{ConsentStore, InMemoryConsentStore, StoreError};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trade_port::TradeAction;

    /// The re-export surface is what the server and the console compile
    /// against, so it is pinned here: renaming or dropping one of these is a
    /// breaking change, not an internal refactor.
    #[test]
    fn the_public_surface_is_the_one_the_console_uses() {
        let bounds: ConsentBounds = ConsentBounds::default();
        assert!(bounds.validate().is_ok());
        assert_eq!(CONSENT_PHRASE, "I CONSENT TO TELEGRAM TRADING");
        // Every re-exported name is reachable through the module root.
        let _: fn(&str) -> [u8; 32] = phrase_sha256;
        let _: fn(&str) -> bool = phrase_matches;
        let _: fn(&Consent, &crate::trade_port::OrgRiskLimits) -> u64 = effective_order_cap;
        let _: fn(&Consent, &crate::trade_port::OrgRiskLimits) -> u64 = effective_daily_cap;
        // `require_active` is generic over the borrow's lifetime, so it is
        // named through a call rather than coerced to a `for<'a>` fn pointer.
        let none: Option<&Consent> = None;
        assert!(require_active(none, TradeAction::Buy, chrono::Utc::now()).is_err());
        let _ = ConsentRequest::default();
        let _ = ConsentScope::both();
        let _ = ConsentStatus::Active;
        let _ = ConsentError::PhraseMismatch;
        let _: InMemoryConsentStore = InMemoryConsentStore::new();
        let _ = StoreError::Conflict("x".to_string());
    }
}
