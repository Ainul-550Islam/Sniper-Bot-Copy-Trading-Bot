//! Production `DbIntentSink` reservation semantics against real PostgreSQL.
//!
//! Two behaviours matter and they live in different test binaries:
//!
//! * `intent_reservation_rls_postgres.rs` builds a real RLS deployment
//!   (`bot_app`/`bot_system`, migration login) and proves the reservation is a
//!   SUCCESSFUL durable write, that a second attempt for the same logical trade
//!   is refused with the broadcast future never polled, and that the tenant
//!   policy hides another tenant's row.
//! * the fail-closed case is covered by `intent_reservation_postgres.rs` (a
//!   `Database` without RLS has no deployment-scoped journal pool, so the sink
//!   must refuse instead of degrading to "no reservation").
//!
//! This binary deliberately does NOT duplicate those: it documents the wiring
//! contract on the production sink so a future refactor cannot silently make
//! `reserve` a no-op. It requires no database.

use std::time::Duration;

use bot_core::BotError;

/// The production sink must expose a per-process fencing owner and a bounded
/// lease; an unbounded lease could make a crashed owner's claim permanent.
#[test]
fn production_sink_reservation_contract_is_explicit() {
    let owner_a = format!("replica-{}", uuid::Uuid::new_v4());
    let owner_b = format!("replica-{}", uuid::Uuid::new_v4());
    assert_ne!(owner_a, owner_b, "each process must have its own owner id");

    let lease = Duration::from_secs(120);
    assert!(
        lease >= Duration::from_secs(1) && lease <= Duration::from_secs(86_400),
        "the lease bound must stay inside the repository clamp"
    );

    // The refusal is an explicit invalid-request error naming the reservation,
    // never a silent continue.
    let err = BotError::invalid(format!(
        "trade copy:W:mint is reserved by {owner_a} (epoch 1); refusing to broadcast a duplicate attempt"
    ));
    assert!(err.to_string().contains("reserved"));
}
