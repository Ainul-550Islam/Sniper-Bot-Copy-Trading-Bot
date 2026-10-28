//! BATCH 04 — real-Postgres stores for Telegram trading (migrations 0066/0067).
//!
//! Gated on `POSTGRES_URL`, exactly like the rest of the suite: with a database
//! these run for real, without one they skip — **except** when
//! `REQUIRE_POSTGRES=1` is set, in which case a missing URL PANICS rather than
//! reporting a green run against nothing.

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use chrono::{DateTime, Duration, Utc};
use module_telegram::commands::audit::{AuditDetail, AuditEvent, AuditStore, TradeAuditEvent};
use module_telegram::commands::pending::{CommandStatus, CommandStore, PendingSpec, TradeCommand};
use module_telegram::consent::store::{ConsentStore, StoreError};
use module_telegram::consent::{draft_consent, ConsentBounds, ConsentRequest, GrantContext};
use module_telegram::trade_port::{OrgRiskLimits, TradeAction};
use module_telegram::CONSENT_PHRASE;
use sniper_suite::telegram_trade_store_pg::PgTradeStore;
use sqlx::Row;
use uuid::Uuid;

fn pg_url() -> Option<String> {
    match std::env::var("POSTGRES_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            if std::env::var("REQUIRE_POSTGRES").as_deref() == Ok("1") {
                panic!("REQUIRE_POSTGRES=1 but POSTGRES_URL is missing");
            }
            None
        }
    }
}

async fn setup() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 8,
        acquire_timeout_ms: 60_000,
        statement_timeout_ms: 120_000,
        query_timeout_ms: 120_000,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("connect postgres"),
    );
    db.migrate().await.expect("migrations must apply");
    Some(db)
}

/// A tenant key unique to one test, so suites never see each other's rows.
fn fresh_org() -> String {
    Uuid::new_v4().to_string()
}

/// A chat id unique to one test.
fn fresh_chat() -> i64 {
    // Positive, fits bigint, far away from real Telegram ids.
    i64::from(Uuid::new_v4().as_u128() as u32) + 7_000_000_000
}

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).expect("timestamp")
}

/// Tenant isolation is proved for real when the Batch 01 foundation is present.
/// With `REQUIRE_RLS=1` a database that cannot enforce it is a FAILURE, never a
/// silent skip (same rule as `REQUIRE_POSTGRES`).
fn require_rls() -> bool {
    std::env::var("REQUIRE_RLS").as_deref() == Ok("1")
}

fn consent_for(org: &str, chat: i64, now: DateTime<Utc>) -> module_telegram::Consent {
    draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest::default(),
        &GrantContext {
            organization_id: org,
            user_id: 111,
            chat_id: chat,
            now,
            org_limits: OrgRiskLimits::new(5_000_000_000, 20_000_000_000),
            phrase: CONSENT_PHRASE,
        },
    )
    .expect("consent drafts")
}

fn spec(org: &str, chat: i64) -> PendingSpec {
    PendingSpec {
        organization_id: org.to_string(),
        user_id: 111,
        chat_id: chat,
        action: TradeAction::Buy,
        mint: "So11111111111111111111111111111111111111112".to_string(),
        amount_lamports: Some(10_000_000),
        percent_bps: None,
        slippage_bps: 100,
    }
}

#[tokio::test]
async fn consents_round_trip_and_never_double_go_live() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let consents = store.consents();
    let org = fresh_org();
    let chat = fresh_chat();

    assert!(consents.active(&org, chat, at(0)).await.unwrap().is_none());

    let first = consent_for(&org, chat, at(0));
    consents.insert(&first).await.expect("insert first consent");
    let live = consents
        .active(&org, chat, at(10))
        .await
        .unwrap()
        .expect("live consent");
    assert_eq!(live.consent_id, first.consent_id);
    assert_eq!(live.max_order_lamports, first.max_order_lamports);
    assert_eq!(live.daily_cap_lamports, first.daily_cap_lamports);
    assert_eq!(live.scope, first.scope);
    assert_eq!(live.phrase_sha256, first.phrase_sha256);
    assert_eq!(store.consents().live_count(&org, chat).await.unwrap(), 1);

    // A second grant while the first is live is a conflict, not a silent
    // overwrite.
    let second = consent_for(&org, chat, at(20));
    assert!(matches!(
        consents.insert(&second).await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(store.consents().live_count(&org, chat).await.unwrap(), 1);

    // Revocation is idempotent.
    let revoked = consents
        .revoke(&org, chat, at(30))
        .await
        .unwrap()
        .expect("revoked row");
    assert_eq!(revoked.consent_id, first.consent_id);
    assert!(revoked.revoked_at.is_some());
    assert!(consents.revoke(&org, chat, at(31)).await.unwrap().is_none());
    assert!(consents.active(&org, chat, at(32)).await.unwrap().is_none());

    // Re-granting after a revoke is allowed and leaves exactly one live row.
    consents.insert(&second).await.expect("re-grant");
    assert_eq!(store.consents().live_count(&org, chat).await.unwrap(), 1);
    assert_eq!(
        consents
            .active(&org, chat, at(40))
            .await
            .unwrap()
            .unwrap()
            .consent_id,
        second.consent_id
    );
    let history = store.consents().history(&org, 10).await.unwrap();
    assert_eq!(history.len(), 2);

    // Another tenant cannot see any of it.
    let other = fresh_org();
    assert!(consents
        .active(&other, chat, at(40))
        .await
        .unwrap()
        .is_none());
    assert!(store
        .consents()
        .history(&other, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_lapsed_consent_is_retired_by_the_next_grant() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let consents = store.consents();
    let org = fresh_org();
    let chat = fresh_chat();

    let first = consent_for(&org, chat, at(0));
    consents.insert(&first).await.expect("insert");
    // 48h later the 24h consent has lapsed: granting again retires it.
    let lapsed = at(48 * 3_600);
    let second = consent_for(&org, chat, lapsed);
    consents
        .insert(&second)
        .await
        .expect("insert lapsed successor");
    assert_eq!(store.consents().live_count(&org, chat).await.unwrap(), 1);
    let history = store.consents().history(&org, 10).await.unwrap();
    assert_eq!(history.len(), 2);
    // Newest first, and the predecessor is stamped revoked.
    assert_eq!(history[0].consent_id, second.consent_id);
    assert!(history[1].revoked_at.is_some());
}

#[tokio::test]
async fn a_non_uuid_tenant_key_is_refused_not_guessed() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let err = store
        .consents()
        .active("org-1", 100, at(0))
        .await
        .expect_err("must refuse");
    assert_eq!(err.code(), "store_unavailable");
    assert!(store
        .commands()
        .count_since("org-1", 100, at(0))
        .await
        .is_err());
    let event = TradeAuditEvent::new(
        None,
        "org-1",
        100,
        AuditEvent::Denied,
        AuditDetail::new().reason("x"),
        at(0),
    );
    assert!(store.audit().append(&event).await.is_err());
}

#[tokio::test]
async fn commands_round_trip_and_only_one_may_be_pending() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let commands = store.commands();
    let org = fresh_org();
    let chat = fresh_chat();

    let (first, nonce) = TradeCommand::open(spec(&org, chat), at(1_000));
    commands.insert(&first).await.expect("insert pending");
    let loaded = commands.get(first.command_id).await.unwrap().expect("row");
    assert_eq!(loaded.status, CommandStatus::Pending);
    assert_eq!(loaded.mint, first.mint);
    assert_eq!(loaded.amount_lamports, Some(10_000_000));
    assert_eq!(loaded.slippage_bps, 100);
    assert_eq!(loaded.idempotency_key, first.command_id);
    assert_eq!(loaded.nonce_sha256, nonce.sha256());

    // The nonce digest is the only lookup key, and it is tenant+chat scoped.
    assert_eq!(
        commands
            .find_by_nonce(&org, chat, &nonce.sha256())
            .await
            .unwrap()
            .expect("by nonce")
            .command_id,
        first.command_id
    );
    assert!(commands
        .find_by_nonce(&fresh_org(), chat, &nonce.sha256())
        .await
        .unwrap()
        .is_none());
    assert!(commands
        .find_by_nonce(&org, chat + 1, &nonce.sha256())
        .await
        .unwrap()
        .is_none());

    // One pending command per chat (partial unique index).
    let (second, _) = TradeCommand::open(spec(&org, chat), at(1_001));
    match commands.insert(&second).await {
        Err(StoreError::Conflict(_)) => {}
        other => panic!("a second pending command must conflict, got {other:?}"),
    }
    // …but another chat of the same tenant is unaffected.
    let other_chat = chat + 1;
    let (third, _) = TradeCommand::open(spec(&org, other_chat), at(1_001));
    commands.insert(&third).await.expect("other chat");

    assert_eq!(
        commands.pending(&org, chat, at(1_002)).await.unwrap().len(),
        1
    );

    // Compare-and-set: the first confirmation wins, the second does not.
    assert!(commands
        .transition(
            first.command_id,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(1_003),
            None
        )
        .await
        .unwrap());
    assert!(!commands
        .transition(
            first.command_id,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(1_004),
            None
        )
        .await
        .unwrap());
    assert!(commands
        .transition(
            first.command_id,
            CommandStatus::Confirmed,
            CommandStatus::Executed,
            at(1_005),
            None
        )
        .await
        .unwrap());
    // Failed is recorded with its code.
    assert!(commands
        .transition(
            third.command_id,
            CommandStatus::Pending,
            CommandStatus::Failed,
            at(1_006),
            Some("port_unavailable")
        )
        .await
        .unwrap());
    assert_eq!(
        commands
            .get(third.command_id)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .as_deref(),
        Some("port_unavailable")
    );
    assert_eq!(
        commands
            .count_in_status(&org, CommandStatus::Executed)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn spent_and_count_only_see_this_chats_committed_buys() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let commands = store.commands();
    let org = fresh_org();
    let chat = fresh_chat();
    // Everything this test writes is stamped from 1_000 onward.
    let since = at(0);

    // Pending does not consume the rolling cap; confirmed and executed do.
    let (pending, _) = TradeCommand::open(spec(&org, chat), at(1_000));
    commands.insert(&pending).await.expect("pending");
    assert_eq!(commands.spent_since(&org, chat, since).await.unwrap(), 0);
    assert_eq!(
        commands.count_since(&org, chat, since).await.unwrap(),
        1,
        "the rate limiter counts every command"
    );

    for (id, status) in [
        (pending.command_id, CommandStatus::Confirmed),
        (pending.command_id, CommandStatus::Executed),
    ] {
        let from = if status == CommandStatus::Confirmed {
            CommandStatus::Pending
        } else {
            CommandStatus::Confirmed
        };
        assert!(commands
            .transition(id, from, status, at(1_500), None)
            .await
            .unwrap());
    }
    assert_eq!(
        commands.spent_since(&org, chat, since).await.unwrap(),
        10_000_000
    );
    // A sell never contributes lamports to the rolling spend.
    let mut sell_spec = spec(&org, chat + 1);
    sell_spec.action = TradeAction::Sell;
    sell_spec.amount_lamports = None;
    sell_spec.percent_bps = Some(5_000);
    let (sell, _) = TradeCommand::open(sell_spec, at(2_000));
    commands.insert(&sell).await.expect("sell");
    assert!(commands
        .transition(
            sell.command_id,
            CommandStatus::Pending,
            CommandStatus::Confirmed,
            at(2_100),
            None
        )
        .await
        .unwrap());
    assert!(commands
        .transition(
            sell.command_id,
            CommandStatus::Confirmed,
            CommandStatus::Executed,
            at(2_200),
            None
        )
        .await
        .unwrap());
    assert_eq!(
        commands.spent_since(&org, chat + 1, since).await.unwrap(),
        0
    );

    // A window far in the future sees nothing.
    assert_eq!(
        commands
            .spent_since(&org, chat, at(9_000_000))
            .await
            .unwrap(),
        0
    );
    // Another tenant sees nothing.
    assert_eq!(
        commands
            .spent_since(&fresh_org(), chat, since)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn cancellation_and_expiry_stamp_every_pending_row() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let commands = store.commands();
    let org = fresh_org();
    let chat = fresh_chat();

    // `expire_overdue` is deliberately GLOBAL (it is a sweeper), so this test
    // keeps its timestamps in the 0..1_000 band: a sweep at t < 1_000 can never
    // reach the rows another test in this binary created at t ≥ 1_000.
    let (live, _) = TradeCommand::open(spec(&org, chat), at(0));
    commands.insert(&live).await.expect("insert live");
    let cancelled = commands.cancel_pending(&org, chat, at(10)).await.unwrap();
    assert_eq!(cancelled, vec![live.command_id]);
    assert_eq!(
        commands.get(live.command_id).await.unwrap().unwrap().status,
        CommandStatus::Cancelled
    );
    // Idempotent.
    assert!(commands
        .cancel_pending(&org, chat, at(11))
        .await
        .unwrap()
        .is_empty());

    // Anything whose TTL has passed is swept to expired.
    let (old, _) = TradeCommand::open(spec(&org, chat + 1), at(100));
    commands.insert(&old).await.expect("insert old");
    let expires = old.expires_at;
    assert!(commands
        .expire_overdue(expires - Duration::seconds(1))
        .await
        .unwrap()
        .iter()
        .all(|id| *id != old.command_id));
    let swept = commands
        .expire_overdue(expires + Duration::seconds(1))
        .await
        .unwrap();
    assert!(swept.contains(&old.command_id));
    assert_eq!(
        commands.get(old.command_id).await.unwrap().unwrap().status,
        CommandStatus::Expired
    );
    // A second sweep finds nothing left to do for that row.
    assert!(!commands
        .expire_overdue(expires + Duration::seconds(2))
        .await
        .unwrap()
        .contains(&old.command_id));
    // History is ordered oldest first and keeps every transition.
    let history = store.commands().history(&org, chat + 1, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, CommandStatus::Expired);
}

#[tokio::test]
async fn audit_rows_are_append_only_and_drop_non_whitelisted_keys() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let audit = store.audit();
    let org = fresh_org();
    let chat = fresh_chat();
    let command_id = Uuid::new_v4();

    let requested = TradeAuditEvent::new(
        Some(command_id),
        &org,
        chat,
        AuditEvent::Requested,
        AuditDetail::new()
            .action(TradeAction::Buy)
            .lamports(
                module_telegram::commands::audit::AuditAmount::AmountLamports,
                10_000_000,
            )
            .mint("So11111111111111111111111111111111111111112"),
        at(7_000),
    );
    audit.append(&requested).await.expect("append requested");
    audit
        .append(&TradeAuditEvent::new(
            Some(command_id),
            &org,
            chat,
            AuditEvent::Executed,
            AuditDetail::new().reason("ok"),
            at(7_001),
        ))
        .await
        .expect("append executed");

    let timeline = store.audit().for_command(command_id).await.unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0].event, AuditEvent::Requested);
    assert_eq!(timeline[1].event, AuditEvent::Executed);
    assert_eq!(timeline[0].chat_id, chat);
    assert!(
        timeline[0].audit_id.is_some(),
        "the store assigns the audit id"
    );

    let recent = audit.recent(&org, 10).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(store.audit().count(&org).await.unwrap(), 2);
    // Another tenant sees none of it.
    assert!(audit.recent(&fresh_org(), 10).await.unwrap().is_empty());

    // A hand-written row carrying a secret-looking key is filtered on read.
    sqlx::query(
        "INSERT INTO telegram_trade_audit (organization_id, chat_id, command_id, event, detail, ts) \
         VALUES ($1, $2, $3, 'denied', $4, $5)",
    )
    .bind(Uuid::parse_str(&org).unwrap())
    .bind(chat)
    .bind(command_id)
    .bind(serde_json::json!({
        "reason": "order_cap_exceeded",
        "nonce": "0123456789abcdef0123456789abcdef",
        "bot_token": "123:fake"
    }))
    .bind(at(7_002))
    .execute(db.pool())
    .await
    .expect("raw insert");
    let rows = audit.recent(&org, 10).await.unwrap();
    assert_eq!(rows.len(), 3);
    let smuggled = &rows[0].detail;
    assert_eq!(
        smuggled.as_map().get("reason").and_then(|v| v.as_str()),
        Some("order_cap_exceeded")
    );
    assert!(smuggled.as_map().get("nonce").is_none());
    assert!(smuggled.as_map().get("bot_token").is_none());
    let rendered = format!("{:?}", rows[0]);
    assert!(!rendered.contains("0123456789abcdef"));

    // The database refuses unknown events outright.
    let bad = sqlx::query(
        "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, detail, ts) \
         VALUES ($1, $2, 'teleported', '{}'::jsonb, $3)",
    )
    .bind(Uuid::parse_str(&org).unwrap())
    .bind(chat)
    .bind(at(7_003))
    .execute(db.pool())
    .await;
    assert!(bad.is_err());
}

/// Pick a role under which row-level security is actually *enforced*.
///
/// Superusers and `BYPASSRLS` roles ignore policies even when they are
/// `FORCE`d, so asserting isolation as `postgres` would prove nothing — the
/// policy would simply not run. The deployment runtime connects as a
/// non-bypass role (`bot_app`, created by migration 0060), so the test switches
/// to that role when it is available and, failing that, creates a throwaway
/// non-bypass role with exactly the privileges the console needs.
///
/// Returns `(role, created_here)`; `created_here` tells the caller to drop it.
async fn rls_probe_role(pool: &sqlx::PgPool) -> Option<(String, bool)> {
    let enforced: bool = sqlx::query(
        "SELECT NOT (rolsuper OR rolbypassrls) AS enforced FROM pg_roles \
         WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .expect("current_user attributes")
    .get("enforced");
    if enforced {
        // Already a non-bypass role: policies are live, nothing to switch to.
        return None;
    }

    let reusable: bool = sqlx::query(
        "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app' AND NOT rolbypassrls) \
            AND has_table_privilege('bot_app', 'public.telegram_trade_consents', 'SELECT') \
            AND has_table_privilege('bot_app', 'public.telegram_trade_consents', 'INSERT') \
            AND has_table_privilege('bot_app', 'public.telegram_trade_audit', 'SELECT') \
            AND has_table_privilege('bot_app', 'public.telegram_trade_audit', 'INSERT') \
            AS ok",
    )
    .fetch_one(pool)
    .await
    .expect("bot_app probe")
    .get("ok");
    if reusable {
        return Some(("bot_app".to_string(), false));
    }

    sqlx::query(
        "DO $$ BEGIN \
             IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'batch04_rls_probe') THEN \
                 CREATE ROLE batch04_rls_probe NOLOGIN NOBYPASSRLS; \
             END IF; \
         END $$",
    )
    .execute(pool)
    .await
    .expect("create probe role");
    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_consents, \
             public.telegram_trade_commands, public.telegram_trade_audit \
         TO batch04_rls_probe",
    )
    .execute(pool)
    .await
    .expect("grant probe role");
    sqlx::query("GRANT EXECUTE ON FUNCTION app.current_org_id() TO batch04_rls_probe")
        .execute(pool)
        .await
        .expect("grant tenant function");
    Some(("batch04_rls_probe".to_string(), true))
}

/// Org A must see exactly its own rows, and an unset tenant context must see
/// nothing at all.
///
/// `FORCE ROW LEVEL SECURITY` is what makes this meaningful: even the table
/// owner cannot escape the policy. The tenants are driven the way the server
/// drives them — `set_config('app.org_id', …, true)` inside a transaction —
/// because that is the exact path a request takes.
#[tokio::test]
async fn org_a_cannot_read_or_update_org_b_rows() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let chat_a = fresh_chat();
    let chat_b = fresh_chat();

    let foundation: bool =
        sqlx::query("SELECT to_regprocedure('app.current_org_id()') IS NOT NULL AS ok")
            .fetch_one(db.pool())
            .await
            .expect("regprocedure")
            .get("ok");
    if !foundation {
        assert!(
            !require_rls(),
            "REQUIRE_RLS=1 but app.current_org_id() is missing: tenant isolation cannot be proved"
        );
        // Without the foundation the migration's guard stays inert by design
        // (it never half-installs a policy), and the store still refuses to
        // serve rows to the wrong tenant because every query is keyed on
        // organization_id.
        insert_tenant_rows(&store, org_a, chat_a).await;
        insert_tenant_rows(&store, org_b, chat_b).await;
        assert!(store
            .consents()
            .active(&org_a.to_string(), chat_b, at(1_100))
            .await
            .expect("store read")
            .is_none());
        assert!(store
            .audit()
            .recent(&org_a.to_string(), 50)
            .await
            .expect("audit read")
            .iter()
            .all(|row| row.chat_id != chat_b));
        return;
    }

    let probe = rls_probe_role(db.pool()).await;
    let role_clause = match &probe {
        Some((role, _)) => format!("SET LOCAL ROLE {role}; "),
        None => String::new(),
    };

    // Rows are written under each tenant's own context, exactly like the bot
    // does when a command arrives in a linked chat.
    for (org, chat) in [(org_a, chat_a), (org_b, chat_b)] {
        let mut tx = db.pool().begin().await.expect("begin");
        sqlx::raw_sql(&format!(
            "{role_clause}SELECT set_config('app.org_id', '{}', true)",
            org
        ))
        .execute(&mut *tx)
        .await
        .expect("set tenant context");
        sqlx::query(
            "INSERT INTO telegram_trade_consents \
             (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
              daily_cap_lamports, granted_at, expires_at, phrase_sha256) \
             VALUES ($1, $2, 111, $3, ARRAY['buy'], 50000000, 200000000, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(chat)
        .bind(at(1_000))
        .bind(at(90_000))
        .bind(vec![7u8; 32])
        .execute(&mut *tx)
        .await
        .expect("insert consent as tenant");
        sqlx::query(
            "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, detail, ts) \
             VALUES ($1, $2, 'denied', '{\"reason\":\"probe\"}'::jsonb, $3)",
        )
        .bind(org)
        .bind(chat)
        .bind(at(1_000))
        .execute(&mut *tx)
        .await
        .expect("insert audit as tenant");
        tx.commit().await.expect("commit tenant rows");
    }

    // 1. Tenant A's context cannot see tenant B's rows.
    let mut tx = db.pool().begin().await.expect("begin");
    sqlx::raw_sql(&format!(
        "{role_clause}SELECT set_config('app.org_id', '{}', true)",
        org_a
    ))
    .execute(&mut *tx)
    .await
    .expect("set tenant A");
    let chats: Vec<i64> =
        sqlx::query("SELECT chat_id FROM telegram_trade_consents ORDER BY chat_id")
            .fetch_all(&mut *tx)
            .await
            .expect("read as tenant A")
            .iter()
            .map(|r| r.get::<i64, _>("chat_id"))
            .collect();
    assert_eq!(
        chats,
        vec![chat_a],
        "tenant A must only see its own consent (role: {:?})",
        probe.as_ref().map(|(role, _)| role.as_str())
    );
    let audits: Vec<i64> = sqlx::query("SELECT chat_id FROM telegram_trade_audit")
        .fetch_all(&mut *tx)
        .await
        .expect("read audit as tenant A")
        .iter()
        .map(|r| r.get::<i64, _>("chat_id"))
        .collect();
    assert_eq!(audits, vec![chat_a], "tenant A must only see its own audit");

    // 2. Tenant A cannot update tenant B's row: the USING clause filters it out,
    //    so the UPDATE affects zero rows rather than silently succeeding.
    let updated =
        sqlx::query("UPDATE telegram_trade_consents SET revoked_at = $1 WHERE chat_id = $2")
            .bind(at(2_000))
            .bind(chat_b)
            .execute(&mut *tx)
            .await
            .expect("update attempt as tenant A");
    assert_eq!(
        updated.rows_affected(),
        0,
        "cross-tenant UPDATE must be a no-op"
    );

    // 3. Tenant A cannot insert a row that claims tenant B (WITH CHECK).
    let smuggled = sqlx::query(
        "INSERT INTO telegram_trade_consents \
         (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
          daily_cap_lamports, granted_at, expires_at, phrase_sha256) \
         VALUES ($1, $2, 111, $3, ARRAY['buy'], 1, 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(org_b)
    .bind(chat_a)
    .bind(at(1_000))
    .bind(at(90_000))
    .bind(vec![9u8; 32])
    .execute(&mut *tx)
    .await;
    assert!(
        smuggled.is_err(),
        "cross-tenant INSERT must be refused by the WITH CHECK clause"
    );
    tx.rollback().await.expect("rollback probe");

    // 4. An unset tenant context sees nothing (fail closed, never "all rows").
    let mut tx = db.pool().begin().await.expect("begin");
    sqlx::raw_sql(&format!(
        "{role_clause}SELECT set_config('app.org_id', '', true)"
    ))
    .execute(&mut *tx)
    .await
    .expect("clear tenant");
    assert_eq!(
        sqlx::query("SELECT count(*) AS n FROM telegram_trade_consents")
            .fetch_one(&mut *tx)
            .await
            .expect("count without tenant")
            .get::<i64, _>("n"),
        0,
        "no tenant context must expose no rows"
    );
    tx.rollback().await.expect("rollback");

    // 5. The application store sees only its own tenant, because every query is
    //    keyed on organization_id. (The store runs on the request pool, whose
    //    role is enforced the same way.)
    let mut tx = db.pool().begin().await.expect("begin");
    sqlx::raw_sql(&format!(
        "{role_clause}SELECT set_config('app.org_id', '{}', true)",
        org_a
    ))
    .execute(&mut *tx)
    .await
    .expect("set tenant A again");
    let own = sqlx::query("SELECT chat_id, organization_id FROM telegram_trade_consents")
        .fetch_all(&mut *tx)
        .await
        .expect("store-equivalent read");
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].get::<i64, _>("chat_id"), chat_a);
    assert_eq!(own[0].get::<Uuid, _>("organization_id"), org_a);
    tx.rollback().await.expect("rollback");

    if let Some((role, true)) = probe {
        sqlx::query(&format!("DROP OWNED BY {role}"))
            .execute(db.pool())
            .await
            .expect("revoke probe privileges");
        sqlx::query(&format!("DROP ROLE IF EXISTS {role}"))
            .execute(db.pool())
            .await
            .expect("drop probe role");
    }
}

/// Insert one consent + one audit row for a tenant through the store (used when
/// the RLS foundation is absent, so there is no context to set).
async fn insert_tenant_rows(store: &PgTradeStore, org: Uuid, chat: i64) {
    let org_id = org.to_string();
    let consent = consent_for(&org_id, chat, at(1_000));
    store
        .consents()
        .insert(&consent)
        .await
        .expect("insert consent");
    store
        .audit()
        .append(&TradeAuditEvent::new(
            None,
            &org_id,
            chat,
            AuditEvent::Denied,
            AuditDetail::new().reason("probe"),
            at(1_000),
        ))
        .await
        .expect("insert audit");
}

#[tokio::test]
async fn the_store_reports_its_own_installation() {
    let Some(db) = setup().await else { return };
    let store = PgTradeStore::new(db.pool().clone());
    assert_eq!(
        store.installed_tables().await.expect("installed tables"),
        3,
        "migrations 0066 and 0067 must have created all three tables"
    );
    // The console consumes exactly this bundle.
    let stores = store.trade_stores();
    assert!(stores.memory().is_none());
    assert!(stores
        .consents
        .active(&fresh_org(), 1, at(0))
        .await
        .unwrap()
        .is_none());
    assert!(stores
        .audit
        .recent(&fresh_org(), 5)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        stores
            .commands
            .count_since(&fresh_org(), 1, at(0))
            .await
            .unwrap(),
        0
    );
}
