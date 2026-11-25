//! Tenant advisory locks never collide across tenants/purposes (gap #69).
//!
//! The helpers used `pg_advisory_xact_lock(hashtext($1))`, i.e. ONE 32-bit key
//! over `"tenant:<uuid>:<purpose>"`. With enough tenants/purposes two unrelated
//! sweeps hash to the same key and block each other (needless contention, and a
//! non-blocking `try_tenant_lock` would report "another worker holds it" for a
//! worker that does not exist). The two-key form
//! `pg_advisory_xact_lock(namespace, hashtext(key))` uses the full 64-bit pair,
//! so a collision needs two identical key strings.
//!
//! Gated on `POSTGRES_URL`; the exact 32-bit collision pair was computed from
//! PostgreSQL's own `hashtext` so the witness is real, not synthetic.

mod trading_isolation_common;

use bot_core::db::tenant_lock;
use bot_core::trading_repository::transaction::TradingTransaction;
use bot_core::trading_repository::TradingQueryScope;
use trading_isolation_common::{org, run_id, setup};

#[tokio::test]
async fn the_two_key_lock_uses_the_full_64_bit_pair() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_advisory_lock_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("lock-a-{run}")).await;
    let b = org(&db, &format!("lock-b-{run}")).await;

    // Brute-force a REAL 32-bit `hashtext` collision between two unrelated
    // tenant keys — the defect the two-key form removes — using PostgreSQL's
    // own hash function (no assumption about the fixture).
    // A REAL 32-bit `hashtext` collision between two tenant keys, found with
    // PostgreSQL's own hash function over real uuid strings (no hand-written
    // fixture): these two tenants' OLD single-key locks are the SAME 32-bit
    // number, so the old code made them contend.
    let collision: Option<(uuid::Uuid, uuid::Uuid, i32)> = sqlx::query_as(
        r#"WITH hashes AS (
             SELECT md5(g::text)::uuid AS u,
                    hashtext('tenant:' || (md5(g::text)::uuid)::text || ':sweep') AS h
               FROM generate_series(1, 200000) g
           ), dup AS (
             SELECT h FROM hashes GROUP BY h HAVING count(*) > 1 LIMIT 1
           )
           SELECT (SELECT min(u::text)::uuid FROM hashes WHERE h = dup.h),
                  (SELECT max(u::text)::uuid FROM hashes WHERE h = dup.h),
                  dup.h
             FROM dup"#,
    )
    .fetch_optional(db.pool())
    .await
    .expect("the collision search runs");
    let (u1, u2, h) = collision.expect("a 32-bit collision exists among 200k tenant keys");

    // Two real tenant rows with exactly those ids.
    for (uuid, tag) in [(u1, "collide-1"), (u2, "collide-2")] {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, status) VALUES ($1,$2,$3,'active')
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(uuid)
        .bind(format!("{tag}-{run}"))
        .bind(format!("{tag} {run}"))
        .execute(db.pool())
        .await
        .expect("insert the colliding organization");
    }
    let c1 = TradingQueryScope::new(bot_core::tenant::OrganizationId::from(u1));
    let c2 = TradingQueryScope::new(bot_core::tenant::OrganizationId::from(u2));

    // Document the collision exactly as PostgreSQL sees it.
    let (old_a, old_b): (i32, i32) = sqlx::query_as(
        "SELECT hashtext('tenant:' || $1::text || ':sweep'), hashtext('tenant:' || $2::text || ':sweep')",
    )
    .bind(u1)
    .bind(u2)
    .fetch_one(db.pool())
    .await
    .expect("hashtext runs");
    assert_eq!(old_a, h);
    assert_eq!(
        old_a, old_b,
        "the fixture is a REAL single-key advisory-lock collision"
    );

    // BEHAVIOUR: tenant 1 holds its sweep; tenant 2 — an unrelated tenant with
    // an unrelated purpose string — must be able to take its own lock. With the
    // old single-key form this returned `false` (a phantom holder).
    let mut holder = TradingTransaction::begin(&db, c1).await.expect("holder tx");
    holder
        .tenant_lock("sweep")
        .await
        .expect("tenant 1 locks its sweep");
    let mut other = TradingTransaction::begin(&db, c2).await.expect("other tx");
    assert!(
        other.try_tenant_lock("sweep").await.expect("try-lock runs"),
        "the colliding tenant is NOT blocked by the two-key form"
    );
    other.rollback().await.expect("other rollback");
    holder.rollback().await.expect("holder rollback");

    // A real tenant transaction holds its own lock; a SECOND tenant's
    // transaction takes its own lock without blocking, and the non-blocking
    // probe from a third connection confirms the two are independent.
    let scope_a = TradingQueryScope::new(a);
    let scope_b = TradingQueryScope::new(b);

    let mut tx_a = TradingTransaction::begin(&db, scope_a).await.expect("tx a");
    tx_a.tenant_lock("recovery-sweep").await.expect("A locks");

    // Same tenant + same purpose from another connection: REFUSED (this is the
    // serialization the lock exists for).
    let mut probe = TradingTransaction::begin(&db, scope_a)
        .await
        .expect("probe");
    assert!(
        !probe
            .try_tenant_lock("recovery-sweep")
            .await
            .expect("try-lock runs"),
        "the same tenant's sweep is serialized"
    );
    // A DIFFERENT tenant is not blocked.
    let mut tx_b = TradingTransaction::begin(&db, scope_b).await.expect("tx b");
    assert!(
        tx_b.try_tenant_lock("recovery-sweep")
            .await
            .expect("try-lock runs"),
        "another tenant's sweep of the same purpose is a different lock"
    );
    // Same tenant, different purpose: also independent.
    let mut probe2 = TradingTransaction::begin(&db, scope_a)
        .await
        .expect("probe2");
    assert!(
        probe2
            .try_tenant_lock("recon-sweep")
            .await
            .expect("try-lock runs"),
        "a different purpose of the same tenant is a different lock"
    );

    probe.rollback().await.expect("probe rollback");
    probe2.rollback().await.expect("probe2 rollback");
    tx_b.rollback().await.expect("b rollback");
    tx_a.commit().await.expect("a commits (releases the lock)");

    // After the holder commits, the same tenant can take the lock again.
    let mut again = TradingTransaction::begin(&db, scope_a)
        .await
        .expect("again");
    assert!(
        again
            .try_tenant_lock("recovery-sweep")
            .await
            .expect("try-lock runs"),
        "the lock is released when the transaction ends"
    );
    again.rollback().await.expect("rollback");

    // And the key helper still separates tenants and purposes.
    let key_a = tenant_lock::tenant_lock_key(&bot_core::db::tenant_lock::lock_scope(a), "x");
    let key_b = tenant_lock::tenant_lock_key(&bot_core::db::tenant_lock::lock_scope(b), "x");
    assert_ne!(key_a, key_b);
}
