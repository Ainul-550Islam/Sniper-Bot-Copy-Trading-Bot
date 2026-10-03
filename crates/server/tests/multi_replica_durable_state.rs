//! Multi-replica regression suite for the durable state introduced by
//! migration 0036 (P0 §S-2, §S-4, and shared WebSocket replay).
//!
//! # What "multi-replica" means in this harness
//!
//! Each "replica" is an INDEPENDENT [`Database`] handle — its own
//! connection pool — over the same PostgreSQL instance, with its own
//! store object built on top. That is exactly the shape two processes
//! behind a load balancer have: separate pools, separate in-process
//! caches, one shared database. Anything these tests prove about replica
//! A and replica B holds for two pods.
//!
//! The one thing a single test process cannot simulate is a separate
//! address space, so the stores' process-wide read-through caches are
//! shared here. That makes these tests STRICTLY HARDER to pass for the
//! cross-replica assertions (a stale cache would hide a missing durable
//! write), and the "restart" test clears the caches explicitly to model a
//! cold process.
//!
//! # Gate
//!
//! Requires `POSTGRES_URL`. Without it every test prints `NOT_RUN` and
//! passes — the same convention the rest of the PG suites use. A CI lane
//! that must not silently skip should assert on the absence of `NOT_RUN`
//! in the output; see `docs/DEPLOYMENT.md`.
//!
//! ```text
//! POSTGRES_URL=postgres://user:pass@localhost/sniper_test \
//!   cargo test -p sniper-suite --test multi_replica_durable_state
//! ```

use std::sync::Arc;
use std::time::Duration;

use bot_core::config::DatabaseConfig;
use bot_core::custody::model::{CustodyProfileId, ProviderType, SignerId};
use bot_core::custody::rotation::{RotationRecord, RotationState};
use bot_core::db::Database;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;
use chrono::Utc;

use sniper_suite::saas::custody_rotation_store::{CustodyRotationStore, RotationStoreError};
use sniper_suite::saas::websocket_replay_store::{ClaimOutcome, WebsocketReplayStore};
use sniper_suite::trading_data_plane::module_control_store::ModuleControlStore;

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn not_run(what: &str) {
    eprintln!("NOT_RUN: {what} — POSTGRES_URL missing");
}

/// Connect one "replica": its own pool, migrated to the current head.
async fn replica() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(Arc::new(db))
}

/// Two independent replicas over one database.
async fn two_replicas() -> Option<(Arc<Database>, Arc<Database>)> {
    let a = replica().await?;
    let b = replica().await?;
    Some((a, b))
}

/// Create a real tenant row — `tenant_module_controls` and
/// `custody_rotations` both carry an `ON DELETE RESTRICT` foreign key to
/// `organizations`, so a synthetic uuid would (correctly) be rejected.
async fn make_org(db: &Database, label: &str) -> OrganizationId {
    let org = OrganizationId::new();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(org.0)
    .bind(format!("{label}-{}", org.0))
    .bind(format!("replica test {label}"))
    .execute(db.pool())
    .await
    .expect("create organization");
    org
}

// ─────────────────────────────────────────────────────────────────────
// 0036 applied
// ─────────────────────────────────────────────────────────────────────

/// The three tables must exist before any of the behaviour below can be
/// meaningful. A failure here means migration 0036 did not apply.
#[tokio::test]
async fn migration_0036_tables_exist() {
    let Some(db) = replica().await else {
        return not_run("0036 tables");
    };
    for table in [
        "tenant_module_controls",
        "custody_rotations",
        "ws_replay_tokens",
    ] {
        let found: Option<(String,)> =
            sqlx::query_as("SELECT tablename FROM pg_tables WHERE tablename = $1")
                .bind(table)
                .fetch_optional(db.pool())
                .await
                .expect("pg_tables");
        assert!(found.is_some(), "migration 0036 missing table {table}");
    }
}

// ─────────────────────────────────────────────────────────────────────
// §S-2 — the tenant kill-switch
// ─────────────────────────────────────────────────────────────────────

/// THE headline regression: a tenant pausing a module on replica A must
/// pause it on replica B. Before 0036 this failed — the pause lived in
/// replica A's RAM and replica B kept trading.
#[tokio::test]
async fn kill_switch_set_on_one_replica_is_honoured_by_the_other() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("kill switch cross-replica");
    };
    let org = make_org(&a, "ks-cross").await;
    let store_a = ModuleControlStore::new(Some(a.clone()));
    let store_b = ModuleControlStore::new(Some(b.clone()));
    assert!(store_a.is_durable() && store_b.is_durable());

    assert_eq!(
        store_b.effective_state(org, BotModule::Sniper).await.0,
        "enabled",
        "no override yet"
    );

    store_a
        .apply_override(
            org,
            BotModule::Sniper,
            false,
            "risk review",
            "user:operator",
            "corr-ks",
            Utc::now(),
        )
        .await
        .expect("replica A writes the pause");

    let (state, entry) = store_b.effective_state(org, BotModule::Sniper).await;
    assert_eq!(state, "disabled", "replica B MUST see the pause");
    let entry = entry.expect("replica B must see the override record");
    assert_eq!(entry.reason, "risk review");
    assert_eq!(entry.updated_by, "user:operator");

    // And clearing it on B must release A.
    assert!(store_b
        .clear_override(org, BotModule::Sniper)
        .await
        .expect("replica B clears"));
    assert_eq!(
        store_a.effective_state(org, BotModule::Sniper).await.0,
        "enabled",
        "replica A MUST see the clear"
    );
}

/// A pause must survive a process restart. Before 0036 a restart silently
/// re-enabled every paused module — the worst possible default.
#[tokio::test]
async fn kill_switch_survives_a_cold_restart() {
    let Some(db) = replica().await else {
        return not_run("kill switch restart");
    };
    let org = make_org(&db, "ks-restart").await;

    ModuleControlStore::new(Some(db.clone()))
        .apply_override(
            org,
            BotModule::Copy,
            false,
            "paused before restart",
            "user:operator",
            "corr-restart",
            Utc::now(),
        )
        .await
        .expect("write");

    // Model a cold process: new pool, and an empty read-through cache.
    sniper_suite::trading_data_plane::module_control_store::reset_cache();
    let fresh = replica().await.expect("reconnect");
    let after = ModuleControlStore::new(Some(fresh));

    let (state, entry) = after.effective_state(org, BotModule::Copy).await;
    assert_eq!(state, "disabled", "the pause MUST survive the restart");
    assert_eq!(entry.expect("entry").reason, "paused before restart");
}

/// Cross-replica durability must not come at the cost of tenant
/// isolation: one tenant's pause is invisible to another tenant on every
/// replica.
#[tokio::test]
async fn kill_switch_stays_tenant_scoped_across_replicas() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("kill switch tenant scope");
    };
    let tenant_a = make_org(&a, "ks-iso-a").await;
    let tenant_b = make_org(&a, "ks-iso-b").await;
    let store_a = ModuleControlStore::new(Some(a.clone()));
    let store_b = ModuleControlStore::new(Some(b.clone()));

    store_a
        .apply_override(
            tenant_a,
            BotModule::Polymarket,
            false,
            "tenant a pause",
            "user:a",
            "",
            Utc::now(),
        )
        .await
        .expect("write");

    assert!(
        store_b
            .override_for(tenant_b, BotModule::Polymarket)
            .await
            .is_none(),
        "tenant B must not inherit tenant A's pause"
    );
    assert_eq!(
        store_b
            .effective_state(tenant_b, BotModule::Polymarket)
            .await
            .0,
        "enabled"
    );
    assert_eq!(
        store_b
            .effective_state(tenant_a, BotModule::Polymarket)
            .await
            .0,
        "disabled"
    );
}

/// Two replicas writing the same control row must not lose an update:
/// the `version` column strictly increases, so the later write is
/// identifiable rather than indistinguishable.
#[tokio::test]
async fn concurrent_control_writes_bump_the_version() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("control versioning");
    };
    let org = make_org(&a, "ks-version").await;
    let store_a = ModuleControlStore::new(Some(a.clone()));
    let store_b = ModuleControlStore::new(Some(b.clone()));

    let first = store_a
        .apply_override(org, BotModule::Sniper, false, "a", "user:a", "", Utc::now())
        .await
        .expect("a writes");
    let second = store_b
        .apply_override(org, BotModule::Sniper, false, "b", "user:b", "", Utc::now())
        .await
        .expect("b writes");

    assert!(
        second.version > first.version,
        "version must increase: {} then {}",
        first.version,
        second.version
    );
    assert_eq!(second.reason, "b");
}

// ─────────────────────────────────────────────────────────────────────
// §S-4 — custody rotation
// ─────────────────────────────────────────────────────────────────────

fn rotation(org: OrganizationId, profile: &str, old: &str, new: &str) -> RotationRecord {
    RotationRecord::new(
        org,
        CustodyProfileId::parse(profile).expect("profile"),
        SignerId::parse(old).expect("old"),
        SignerId::parse(new).expect("new"),
        ProviderType::Local,
        Utc::now(),
    )
}

/// A rotation created on replica A must be readable — and advanceable —
/// on replica B. Before 0036, `POST /rotations/:id/activate` behind a
/// load balancer succeeded or 404'd depending on which pod answered.
#[tokio::test]
async fn rotation_created_on_one_replica_activates_on_the_other() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("rotation cross-replica");
    };
    let org = make_org(&a, "rot-cross").await;
    let store_a = CustodyRotationStore::new(Some(a.clone()));
    let store_b = CustodyRotationStore::new(Some(b.clone()));

    let rec = rotation(org, "profile-cross", "signer-old", "signer-new");
    store_a.insert(&rec).await.expect("replica A creates");

    let mut on_b = store_b
        .get(rec.id)
        .await
        .expect("replica B read")
        .expect("replica B MUST see the rotation");
    assert_eq!(on_b.state, RotationState::Pending);
    assert_eq!(on_b.organization_id, org);

    on_b.transition(RotationState::Active, Utc::now(), false)
        .expect("activate");
    store_b
        .save_transition(&on_b, "user:b", "corr-b")
        .await
        .expect("replica B persists the activation");

    let on_a = store_a
        .get(rec.id)
        .await
        .expect("replica A read")
        .expect("present");
    assert_eq!(
        on_a.state,
        RotationState::Active,
        "replica A MUST see the activation"
    );
}

/// At most one in-flight rotation per (tenant, profile), enforced by the
/// database rather than by whichever replica happens to be asked. This is
/// the race that can revoke a signer another rotation still depends on.
#[tokio::test]
async fn second_inflight_rotation_is_refused_across_replicas() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("rotation in-flight guard");
    };
    let org = make_org(&a, "rot-inflight").await;
    let store_a = CustodyRotationStore::new(Some(a.clone()));
    let store_b = CustodyRotationStore::new(Some(b.clone()));

    store_a
        .insert(&rotation(org, "profile-lock", "signer-old", "signer-new"))
        .await
        .expect("first rotation");

    let second = store_b
        .insert(&rotation(org, "profile-lock", "signer-old", "signer-third"))
        .await;
    assert_eq!(
        second,
        Err(RotationStoreError::ConflictInFlight),
        "replica B must refuse a second in-flight rotation on the same profile"
    );
}

/// The in-flight guard is tenant-composite: two tenants may rotate
/// identically-named profiles at the same time.
#[tokio::test]
async fn rotation_inflight_guard_is_tenant_scoped() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("rotation tenant scope");
    };
    let tenant_a = make_org(&a, "rot-iso-a").await;
    let tenant_b = make_org(&a, "rot-iso-b").await;
    let store_a = CustodyRotationStore::new(Some(a.clone()));
    let store_b = CustodyRotationStore::new(Some(b.clone()));

    store_a
        .insert(&rotation(tenant_a, "profile-shared", "s-old", "s-new"))
        .await
        .expect("tenant A");
    store_b
        .insert(&rotation(tenant_b, "profile-shared", "s-old", "s-new"))
        .await
        .expect("tenant B must not be blocked by tenant A");

    assert_eq!(
        store_b
            .list_for_tenant(tenant_a, 50)
            .await
            .expect("list")
            .len(),
        1,
        "a tenant's listing must contain only its own rotations"
    );
    assert!(store_a
        .list_for_tenant(tenant_a, 50)
        .await
        .expect("list")
        .iter()
        .all(|r| r.organization_id == tenant_a));
}

/// A rotation must survive a cold restart: the custody profile must never
/// be left between signers with no record that a rotation was started.
#[tokio::test]
async fn rotation_survives_a_cold_restart() {
    let Some(db) = replica().await else {
        return not_run("rotation restart");
    };
    let org = make_org(&db, "rot-restart").await;
    let rec = rotation(org, "profile-restart", "signer-old", "signer-new");
    CustodyRotationStore::new(Some(db.clone()))
        .insert(&rec)
        .await
        .expect("insert");

    sniper_suite::saas::custody_rotation_store::reset_cache();
    let fresh = replica().await.expect("reconnect");
    let after = CustodyRotationStore::new(Some(fresh))
        .get(rec.id)
        .await
        .expect("read")
        .expect("the rotation MUST survive the restart");
    assert_eq!(after.state, RotationState::Pending);
    assert_eq!(after.old_signer.to_string(), "signer-old");
}

// ─────────────────────────────────────────────────────────────────────
// Shared WebSocket replay protection
// ─────────────────────────────────────────────────────────────────────

/// A ticket presented to replica A must be refused by replica B. Before
/// 0036 the seen-set was per-process, so a captured ticket replayed
/// successfully simply by reaching a different pod.
#[tokio::test]
async fn websocket_ticket_claimed_on_one_replica_is_a_replay_on_the_other() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("ws replay cross-replica");
    };
    let store_a = WebsocketReplayStore::new(Some(a.clone()));
    let store_b = WebsocketReplayStore::new(Some(b.clone()));
    assert!(store_a.is_shared() && store_b.is_shared());

    let ticket = format!("ticket-{}", uuid::Uuid::new_v4());
    let window = Duration::from_secs(300);

    assert_eq!(
        store_a.claim(&ticket, window, None, "corr-a").await,
        ClaimOutcome::Fresh,
        "first presentation is fresh"
    );
    assert_eq!(
        store_b.claim(&ticket, window, None, "corr-b").await,
        ClaimOutcome::Replay,
        "replica B MUST refuse the replayed ticket"
    );
}

/// Replay detection is deliberately NOT tenant-scoped: a ticket that is
/// replayable once per tenant is still replayable.
#[tokio::test]
async fn websocket_replay_detection_ignores_tenant() {
    let Some((a, b)) = two_replicas().await else {
        return not_run("ws replay tenant scope");
    };
    let tenant_a = make_org(&a, "ws-a").await;
    let tenant_b = make_org(&a, "ws-b").await;
    let store_a = WebsocketReplayStore::new(Some(a.clone()));
    let store_b = WebsocketReplayStore::new(Some(b.clone()));

    let ticket = format!("ticket-{}", uuid::Uuid::new_v4());
    let window = Duration::from_secs(300);

    assert_eq!(
        store_a.claim(&ticket, window, Some(tenant_a), "").await,
        ClaimOutcome::Fresh
    );
    assert_eq!(
        store_b.claim(&ticket, window, Some(tenant_b), "").await,
        ClaimOutcome::Replay
    );
}

/// An expired claim is garbage, not an "already seen" answer: a fresh
/// ticket must still be admitted once its predecessor's window has
/// lapsed, even if the sweeper has not run.
#[tokio::test]
async fn an_expired_claim_does_not_refuse_a_fresh_ticket() {
    let Some(db) = replica().await else {
        return not_run("ws replay expiry");
    };
    let store = WebsocketReplayStore::new(Some(db.clone()));
    let ticket = format!("ticket-{}", uuid::Uuid::new_v4());

    // A zero-length window expires immediately.
    assert_eq!(
        store.claim(&ticket, Duration::from_secs(0), None, "").await,
        ClaimOutcome::Fresh
    );
    assert_eq!(
        store
            .claim(&ticket, Duration::from_secs(300), None, "")
            .await,
        ClaimOutcome::Fresh,
        "an expired claim must be re-claimable without a sweep"
    );
    // ... and the re-claim is itself protected.
    assert_eq!(
        store
            .claim(&ticket, Duration::from_secs(300), None, "")
            .await,
        ClaimOutcome::Replay
    );
}

/// Distinct tickets never collide, and the sweeper removes only expired
/// rows.
#[tokio::test]
async fn sweep_removes_only_expired_claims() {
    let Some(db) = replica().await else {
        return not_run("ws replay sweep");
    };
    let store = WebsocketReplayStore::new(Some(db.clone()));
    let live = format!("live-{}", uuid::Uuid::new_v4());
    let dead = format!("dead-{}", uuid::Uuid::new_v4());

    assert_eq!(
        store.claim(&live, Duration::from_secs(300), None, "").await,
        ClaimOutcome::Fresh
    );
    assert_eq!(
        store.claim(&dead, Duration::from_secs(0), None, "").await,
        ClaimOutcome::Fresh
    );

    store.sweep_expired().await;

    assert_eq!(
        store.claim(&live, Duration::from_secs(300), None, "").await,
        ClaimOutcome::Replay,
        "a live claim must survive the sweep"
    );
    assert_eq!(
        store.claim(&dead, Duration::from_secs(300), None, "").await,
        ClaimOutcome::Fresh,
        "an expired claim is gone after the sweep"
    );
}
