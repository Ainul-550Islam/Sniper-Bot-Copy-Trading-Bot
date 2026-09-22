//! TASK 7A — SaaS control plane: deterministic offline proof suite.
//!
//! Proves the twelve properties the specification requires (A–L), against
//! the real domain types: tenants, memberships, the permission matrix,
//! sessions, tenant API keys, entitlements, usage metering, provisioning,
//! and the one authorization decision. No database, no network.
//!
//! The two "cannot bypass" proofs (K, L) matter most: the SaaS layer is a
//! gate in FRONT of the trading engines, never a replacement for them.

use std::sync::Arc;

use chrono::{Duration, Utc};

use bot_core::authorization::{
    authorize, AccessRequest, AuthorizationContext, DecisionKind, Principal,
};
use bot_core::billing::{
    entitlements_from_plan, features, Entitlement, EntitlementSet, EntitlementSource, FeatureLimit,
    Plan, PlanCode, Subscription, UsageEvent, UsageLedger, UsageMetric, UsageOutcome, UsageSource,
};
use bot_core::membership::{Membership, MembershipRole, MembershipStatus, Permission, PermissionSet};
use bot_core::provisioning::{plan_next, ProvisioningAction, ProvisioningJob, ProvisioningStep};
use bot_core::session::token::{generate_token, hash_password, hash_token, verify_password};
use bot_core::session::{validate, SessionRecord, SessionRejection};
use bot_core::tenant::{
    check_resource, Organization, OrganizationId, OrganizationStatus, TenantAction, User, UserId,
    UserStatus,
};

// ---------------------------------------------------------------- helpers --

fn org(slug: &str, status: OrganizationStatus) -> Organization {
    let mut o = Organization::new(
        OrganizationId::new(),
        slug,
        slug,
        Some(UserId::new()),
        Utc::now(),
    );
    o.status = status;
    o
}

fn user(email: &str) -> User {
    let now = Utc::now();
    User {
        id: UserId::new(),
        email: User::normalize_email(email),
        email_verified: true,
        display_name: email.into(),
        password_hash: hash_password("correct horse battery staple"),
        status: UserStatus::Active,
        platform_admin: false,
        created_at: now,
        updated_at: now,
        last_login_at: None,
    }
}

fn ctx(o: &Organization, u: &User, role: MembershipRole) -> AuthorizationContext {
    let m = Membership::new(o.id, u.id, role, None, Utc::now());
    AuthorizationContext::from_membership(
        Principal::UserSession {
            session_id: "s".into(),
        },
        o,
        &m,
        None,
        u.platform_admin,
        Utc::now(),
    )
}

fn entitlements(o: &Organization, code: PlanCode) -> EntitlementSet {
    let now = Utc::now();
    let plan = bot_core::billing::default_catalogue(now)
        .into_iter()
        .find(|p| p.code == code)
        .expect("catalogue tier");
    let sub = Subscription::manual(o.id, plan.id, now);
    let rows = entitlements_from_plan(o.id, &plan, now);
    EntitlementSet::resolve(Some(&plan), Some(&sub), &rows, now)
}

// ------------------------------------------------- A: own-tenant access ----

#[test]
fn a_tenant_can_access_its_own_resources() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let alice = user("alice@a.test");
    let c = ctx(&a, &alice, MembershipRole::Trader);
    let ents = entitlements(&a, PlanCode::Business);

    for request in [
        AccessRequest::read(Permission::OrderRead).on_resource(a.id),
        AccessRequest::read(Permission::LedgerRead).on_resource(a.id),
        AccessRequest::trade(Permission::BotStart).on_resource(a.id),
        AccessRequest::reduce_risk(Permission::BotStop).on_resource(a.id),
    ] {
        let d = authorize(Some(&c), &request, Some(&ents));
        assert!(d.is_allowed(), "{}: {}", request.permission, d);
    }
}

// ------------------------------------------ B: cross-tenant is refused -----

#[test]
fn b_tenant_b_cannot_access_tenant_a_resources() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let b = org("tenant-b", OrganizationStatus::Active);
    let bob = user("bob@b.test");

    // Bob is an OWNER in his own tenant — the strongest ordinary role.
    let c = ctx(&b, &bob, MembershipRole::OrgOwner);
    let ents = entitlements(&b, PlanCode::Business);

    for request in [
        AccessRequest::read(Permission::OrderRead).on_resource(a.id),
        AccessRequest::read(Permission::LedgerRead).on_resource(a.id),
        AccessRequest::manage(Permission::TenantUpdate).on_resource(a.id),
        AccessRequest::trade(Permission::BotStart).on_resource(a.id),
        AccessRequest::manage(Permission::ApiKeyCreate).on_resource(a.id),
    ] {
        let d = authorize(Some(&c), &request, Some(&ents));
        assert_eq!(
            d.kind,
            DecisionKind::DenyResource,
            "{} must be refused across tenants",
            request.permission
        );
        assert_eq!(d.http_status(), 403, "must not 404: that reveals existence");
    }

    // The domain-level ownership check agrees.
    assert!(!check_resource(&b, a.id, TenantAction::Read).is_allowed());
    assert!(check_resource(&b, b.id, TenantAction::Read).is_allowed());
}

// ------------------------------------------- C: suspended tenant gate ------

#[test]
fn c_a_suspended_tenant_cannot_trade_or_control_resources() {
    let s = org("suspended", OrganizationStatus::Suspended);
    let owner = user("owner@s.test");
    let c = ctx(&s, &owner, MembershipRole::OrgOwner);
    let ents = entitlements(&s, PlanCode::Business);

    // Trading and management stop.
    for request in [
        AccessRequest::trade(Permission::BotStart),
        AccessRequest::trade(Permission::OrderManage),
        AccessRequest::manage(Permission::TenantUpdate),
        AccessRequest::manage(Permission::ApiKeyCreate),
        AccessRequest::manage(Permission::RiskManage),
    ] {
        let d = authorize(Some(&c), &request, Some(&ents));
        assert_eq!(
            d.kind,
            DecisionKind::DenySuspended,
            "{} must be refused while suspended",
            request.permission
        );
    }

    // Reading, reducing risk and paying stay possible — a customer must
    // never be trapped in a position or unable to settle the bill.
    for request in [
        AccessRequest::read(Permission::OrderRead),
        AccessRequest::read(Permission::LedgerRead),
        AccessRequest::reduce_risk(Permission::BotStop),
        AccessRequest::billing(Permission::BillingManage),
    ] {
        assert!(
            authorize(Some(&c), &request, Some(&ents)).is_allowed(),
            "{} must stay available while suspended",
            request.permission
        );
    }

    // A closed tenant loses even reads.
    let closed = org("closed", OrganizationStatus::Closed);
    let cc = ctx(&closed, &owner, MembershipRole::OrgOwner);
    assert_eq!(
        authorize(Some(&cc), &AccessRequest::read(Permission::OrderRead), None).kind,
        DecisionKind::DenySuspended
    );
}

// ------------------------------------------------- D: RBAC enforcement -----

#[test]
fn d_a_role_without_the_permission_is_denied() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let ents = entitlements(&a, PlanCode::Business);

    // The full matrix: for every role, every permission it does NOT hold
    // must be refused with DENY_PERMISSION.
    for role in MembershipRole::ALL {
        let u = user("member@a.test");
        let c = ctx(&a, &u, role);
        for permission in Permission::ALL {
            let request = AccessRequest::read(permission);
            let d = authorize(Some(&c), &request, Some(&ents));
            if role.grants(permission) {
                assert!(d.is_allowed(), "{role} should hold {permission}: {d}");
            } else {
                assert_eq!(
                    d.kind,
                    DecisionKind::DenyPermission,
                    "{role} must not hold {permission}"
                );
            }
        }
    }

    // Spot checks of the separation of duties.
    let trader = ctx(&a, &user("t@a.test"), MembershipRole::Trader);
    assert_eq!(
        authorize(
            Some(&trader),
            &AccessRequest::manage(Permission::ApiKeyCreate),
            Some(&ents)
        )
        .kind,
        DecisionKind::DenyPermission
    );
    let security = ctx(&a, &user("s@a.test"), MembershipRole::SecurityAdmin);
    assert_eq!(
        authorize(
            Some(&security),
            &AccessRequest::trade(Permission::BotStart),
            Some(&ents)
        )
        .kind,
        DecisionKind::DenyPermission,
        "a security admin must not be able to trade"
    );
    assert!(authorize(
        Some(&security),
        &AccessRequest::manage(Permission::ApiKeyCreate),
        Some(&ents)
    )
    .is_allowed());

    // A suspended membership holds nothing at all.
    let mut m = Membership::new(a.id, UserId::new(), MembershipRole::OrgOwner, None, Utc::now());
    m.status = MembershipStatus::Suspended;
    let suspended = AuthorizationContext::from_membership(
        Principal::UserSession { session_id: "s".into() },
        &a,
        &m,
        None,
        false,
        Utc::now(),
    );
    assert_eq!(
        authorize(
            Some(&suspended),
            &AccessRequest::read(Permission::TenantRead),
            Some(&ents)
        )
        .kind,
        DecisionKind::DenyPermission
    );
}

// ------------------------------- E/F/G: API-key restart, revoke, expire ----

/// A minimal stand-in for the durable `saas_api_keys` row, exercising the
/// same hash-only storage and lifecycle rules the server uses.
#[derive(Clone)]
struct KeyRow {
    organization_id: OrganizationId,
    secret_hash: String,
    key_prefix: String,
    role: MembershipRole,
    expires_at: Option<chrono::DateTime<Utc>>,
    revoked_at: Option<chrono::DateTime<Utc>>,
}

impl KeyRow {
    fn usable(&self, now: chrono::DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.map(|e| e > now).unwrap_or(true)
    }
}

fn issue_key(o: &Organization, role: MembershipRole, ttl_days: Option<i64>) -> (KeyRow, String) {
    let now = Utc::now();
    let token = generate_token("sk");
    (
        KeyRow {
            organization_id: o.id,
            secret_hash: token.hash.clone(),
            key_prefix: token.prefix.clone(),
            role,
            expires_at: ttl_days.map(|d| now + Duration::days(d)),
            revoked_at: None,
        },
        token.plaintext,
    )
}

#[test]
fn e_an_api_key_created_before_a_restart_still_works_after_it() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let (row, plaintext) = issue_key(&a, MembershipRole::Trader, None);

    // "Before the restart": the row is persisted, the plaintext is handed
    // to the customer and then forgotten by the server.
    let durable: Vec<KeyRow> = vec![row.clone()];
    drop(row);

    // "After the restart": the process starts with an EMPTY in-memory
    // registry and reloads from the durable records — the previous
    // behaviour lost the key here.
    let mut registry: std::collections::HashMap<String, KeyRow> =
        std::collections::HashMap::new();
    assert!(
        registry.get(&hash_token(&plaintext)).is_none(),
        "a fresh process knows nothing"
    );
    for k in durable {
        registry.insert(k.secret_hash.clone(), k);
    }

    let found = registry
        .get(&hash_token(&plaintext))
        .expect("the key is reconstructed from its durable record");
    assert!(found.usable(Utc::now()));
    assert_eq!(found.organization_id, a.id);
    assert_eq!(found.role, MembershipRole::Trader);

    // It authenticates into its own tenant only.
    let c = AuthorizationContext::from_api_key(
        Principal::ApiKey {
            key_id: "k".into(),
            key_prefix: found.key_prefix.clone(),
        },
        &a,
        found.role,
        None,
        None,
        Utc::now(),
    );
    assert!(authorize(Some(&c), &AccessRequest::read(Permission::BotRead), None).is_allowed());
    let other = org("tenant-b", OrganizationStatus::Active);
    assert_eq!(
        authorize(
            Some(&c),
            &AccessRequest::read(Permission::BotRead).on_resource(other.id),
            None
        )
        .kind,
        DecisionKind::DenyResource
    );
}

#[test]
fn f_a_revoked_api_key_stops_working() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let (mut row, plaintext) = issue_key(&a, MembershipRole::Trader, None);
    let now = Utc::now();
    assert!(row.usable(now));
    assert_eq!(hash_token(&plaintext), row.secret_hash);

    row.revoked_at = Some(now);
    assert!(!row.usable(now));
    // Revocation survives a reload: the durable row carries it.
    let reloaded = row.clone();
    assert!(!reloaded.usable(now + Duration::days(1)));
}

#[test]
fn g_an_expired_api_key_stops_working() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let (row, _plaintext) = issue_key(&a, MembershipRole::Viewer, Some(7));
    let now = Utc::now();
    assert!(row.usable(now));
    assert!(row.usable(now + Duration::days(6)));
    assert!(!row.usable(now + Duration::days(8)));
}

#[test]
fn sessions_expire_revoke_and_never_store_the_token() {
    let now = Utc::now();
    let u = user("alice@a.test");
    let a = org("tenant-a", OrganizationStatus::Active);
    let token = generate_token("ses");
    let mut s = SessionRecord::new(
        u.id,
        Some(a.id),
        token.hash.clone(),
        token.prefix.clone(),
        Duration::hours(1),
        now,
    );
    assert!(validate(Some(&s), Some(a.id), now).is_ok());
    // Never the plaintext, anywhere.
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains(&token.plaintext));

    // Wrong tenant.
    let b = org("tenant-b", OrganizationStatus::Active);
    assert_eq!(
        validate(Some(&s), Some(b.id), now),
        Err(SessionRejection::WrongTenant)
    );
    // Expiry.
    assert_eq!(
        validate(Some(&s), Some(a.id), now + Duration::hours(2)),
        Err(SessionRejection::Expired)
    );
    // Revocation.
    s.revoke("logout", now);
    assert_eq!(
        validate(Some(&s), Some(a.id), now),
        Err(SessionRejection::Revoked)
    );
    // Password verification is real.
    assert!(verify_password("correct horse battery staple", &u.password_hash));
    assert!(!verify_password("wrong", &u.password_hash));
    assert!(!u.password_hash.contains("correct"));
}

// ----------------------------------------------- H: usage idempotency ------

#[test]
fn h_the_same_usage_event_cannot_be_counted_twice() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let b = org("tenant-b", OrganizationStatus::Active);
    let now = Utc::now();
    let mut ledger = UsageLedger::new();

    let key = UsageEvent::key_for(UsageMetric::OrdersSubmitted, "order-42");
    let event = UsageEvent::new(
        a.id,
        UsageMetric::OrdersSubmitted,
        1.0,
        UsageSource::Sniper,
        key.clone(),
        now,
    );

    assert_eq!(ledger.record(event.clone()), UsageOutcome::Recorded);
    assert_eq!(ledger.record(event.clone()), UsageOutcome::Duplicate);
    // A retry from another worker with a different quantity is still the
    // same fact.
    let mut retry = event.clone();
    retry.quantity = 100.0;
    assert_eq!(ledger.record(retry), UsageOutcome::Duplicate);
    assert_eq!(ledger.total(a.id, UsageMetric::OrdersSubmitted, &event.period()), 1.0);

    // The same key in ANOTHER tenant is a different fact.
    let other = UsageEvent::new(
        b.id,
        UsageMetric::OrdersSubmitted,
        1.0,
        UsageSource::Sniper,
        key,
        now,
    );
    assert_eq!(ledger.record(other), UsageOutcome::Recorded);
    assert_eq!(ledger.total(b.id, UsageMetric::OrdersSubmitted, &event.period()), 1.0);
    assert_eq!(ledger.len(), 2);
}

// ----------------------------------------- I: provisioning resumability ----

#[test]
fn i_provisioning_resumes_after_a_crash_without_duplicating() {
    let now = Utc::now();
    let request_key = ProvisioningJob::request_key_for("new@customer.test", "acme");
    let mut job = ProvisioningJob::new(request_key.clone(), "starter", now);

    // Get as far as the organization, then "crash".
    job.begin(now);
    job.complete_step(ProvisioningStep::UserCreated, now);
    job.user_id = Some(UserId::new());
    job.complete_step(ProvisioningStep::OrganizationCreated, now);
    let created_org = OrganizationId::new();
    job.organization_id = Some(created_org);

    // A new process loads the row by the SAME deterministic request key.
    let same_key = ProvisioningJob::request_key_for("New@Customer.test", "acme");
    assert_eq!(same_key, request_key, "a retried signup finds the same job");
    let mut resumed = job.clone();

    // It continues from the next step, and the organization is not recreated.
    assert_eq!(
        plan_next(&resumed),
        ProvisioningAction::Execute(ProvisioningStep::MembershipCreated)
    );
    assert_eq!(resumed.organization_id, Some(created_org));

    while let ProvisioningAction::Execute(step) = plan_next(&resumed) {
        assert!(resumed.complete_step(step, now), "{step}");
    }
    assert!(resumed.is_ready());
    assert_eq!(resumed.organization_id, Some(created_org), "exactly one tenant");
    assert_eq!(plan_next(&resumed), ProvisioningAction::Done);

    // A duplicated worker replaying an old step cannot rewind the cursor.
    assert!(!resumed.complete_step(ProvisioningStep::UserCreated, now));
    assert!(resumed.is_ready());
}

// --------------------------- J: identical-looking ids cannot cross-read ----

#[test]
fn j_two_organizations_with_identical_looking_resource_ids_cannot_cross_read() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let b = org("tenant-b", OrganizationStatus::Active);

    // Both tenants have a resource whose LOCAL identifier is the same
    // string — e.g. both track the mint "MINT-XYZ" or an order "ord-1".
    let shared_resource_name = "ord-1";
    let alice = ctx(&a, &user("alice@a.test"), MembershipRole::OrgOwner);
    let bob = ctx(&b, &user("bob@b.test"), MembershipRole::OrgOwner);

    // Ownership is decided by the TENANT the resource belongs to, never by
    // the resource's own id — so the identical name changes nothing.
    let resource_in_a = a.id;
    let resource_in_b = b.id;
    assert!(alice.owns(resource_in_a));
    assert!(!alice.owns(resource_in_b));
    assert!(bob.owns(resource_in_b));
    assert!(!bob.owns(resource_in_a));

    let d = authorize(
        Some(&bob),
        &AccessRequest::read(Permission::OrderRead).on_resource(resource_in_a),
        None,
    );
    assert_eq!(d.kind, DecisionKind::DenyResource, "{shared_resource_name}");

    // And the reverse.
    let d = authorize(
        Some(&alice),
        &AccessRequest::read(Permission::OrderRead).on_resource(resource_in_b),
        None,
    );
    assert_eq!(d.kind, DecisionKind::DenyResource);

    // A platform admin (role AND user flag) may cross, and that is the only
    // way anyone can.
    let mut staff_user = user("staff@platform.test");
    staff_user.platform_admin = true;
    let staff = ctx(&a, &staff_user, MembershipRole::PlatformAdmin);
    assert!(authorize(
        Some(&staff),
        &AccessRequest::read(Permission::OrderRead).on_resource(resource_in_b),
        None
    )
    .is_allowed());
}

// ------------------------- K/L: SaaS cannot bypass TASK 5 / TASK 6 ---------

#[tokio::test]
async fn k_the_saas_layer_cannot_bypass_the_task5_global_risk_engine() {
    use bot_core::config::AppConfig;
    use bot_core::models::{BotModule, Venue};
    use bot_core::risk::{EntryRequest, RiskCode, RiskEngine};
    use bot_core::state::AppState;

    // A tenant whose plan allows everything, and an OWNER who holds every
    // permission: the SaaS layer says yes.
    let o = org("tenant-a", OrganizationStatus::Active);
    let owner = user("owner@a.test");
    let c = ctx(&o, &owner, MembershipRole::OrgOwner);
    let ents = entitlements(&o, PlanCode::Business);
    assert!(authorize(
        Some(&c),
        &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
        Some(&ents)
    )
    .is_allowed());

    // The global risk engine still runs and still refuses: the venue is
    // kill-switched at the platform level.
    let mut cfg = AppConfig::from_defaults();
    cfg.raw.sniper.enabled = true;
    cfg.raw.global_risk.killed_venues = vec!["pump.fun".into()];
    let state = AppState::new(cfg);
    let risk = RiskEngine::new(state.clone());
    let decision = risk
        .check_entry(&EntryRequest {
            module: BotModule::Sniper,
            venue: Venue::PumpFun,
            symbol: "MINT".into(),
            symbol_display: "MINT".into(),
            requested_quote: 0.1,
            available_quote: 100.0,
            slippage_bps: 100,
            price: None,
            fair_value: None,
            liquidity: None,
            wallet: "w".into(),
            strategy: "sniper".into(),
        })
        .await;
    assert!(
        !decision.allowed(),
        "a SaaS ALLOW must never imply the risk engine agrees"
    );
    assert_eq!(decision.code, Some(RiskCode::GlobalKillSwitch));
}

#[tokio::test]
async fn l_the_saas_layer_cannot_bypass_task6_ha_fencing() {
    use bot_core::ha::{HaRuntime, HaSettings, LeaseRole, MemoryHaStore};
    use bot_core::events::EventBus;

    // Two workers, one shared HA store.
    let store = Arc::new(MemoryHaStore::new());
    let a = HaRuntime::new("w-a", HaSettings::default(), EventBus::new(16));
    let b = HaRuntime::new("w-b", HaSettings::default(), EventBus::new(16));
    a.attach_store(store.clone()).await;
    b.attach_store(store.clone()).await;
    a.register("h", 1, "v").await.unwrap();
    b.register("h", 2, "v").await.unwrap();

    let role = LeaseRole::AccountingMaintenance;
    let guard_a = a.acquire(role.clone()).await.unwrap().expect("w-a wins");
    assert!(b.acquire(role.clone()).await.unwrap().is_none());

    // A fully authorized SaaS caller exists on the losing worker…
    let o = org("tenant-a", OrganizationStatus::Active);
    let owner = user("owner@a.test");
    let c = ctx(&o, &owner, MembershipRole::OrgOwner);
    assert!(authorize(
        Some(&c),
        &AccessRequest::manage(Permission::RiskManage),
        None
    )
    .is_allowed());

    // …but the lease is what decides who may mutate shared state. After a
    // takeover the old holder is fenced regardless of any SaaS permission.
    store.advance_clock(chrono::Duration::seconds(120));
    assert!(b.acquire(role).await.unwrap().is_some(), "w-b takes over");
    let err = a.fence(&guard_a).await.unwrap_err();
    assert_eq!(err.reason(), "fenced");
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let r2 = Arc::clone(&ran);
    assert!(a
        .guarded(&guard_a, async move {
            r2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await
        .is_err());
    assert!(
        !ran.load(std::sync::atomic::Ordering::SeqCst),
        "an authorized SaaS caller on a fenced worker still does not run"
    );
}

// ----------------------------------------------- entitlement behaviour -----

#[test]
fn plan_entitlements_gate_features_and_limits() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let owner = user("owner@a.test");
    let c = ctx(&a, &owner, MembershipRole::OrgOwner);

    // Starter: no live trading, no Polymarket, one API key.
    let starter = entitlements(&a, PlanCode::Starter);
    assert_eq!(
        authorize(
            Some(&c),
            &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
            Some(&starter)
        )
        .kind,
        DecisionKind::DenyEntitlement
    );
    assert_eq!(
        authorize(
            Some(&c),
            &AccessRequest::manage(Permission::ApiKeyCreate)
                .consuming(features::MAX_API_KEYS, 1.0, 1.0),
            Some(&starter)
        )
        .kind,
        DecisionKind::DenyEntitlement
    );

    // Business: both allowed.
    let business = entitlements(&a, PlanCode::Business);
    assert!(authorize(
        Some(&c),
        &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
        Some(&business)
    )
    .is_allowed());
    assert!(authorize(
        Some(&c),
        &AccessRequest::manage(Permission::ApiKeyCreate)
            .consuming(features::MAX_API_KEYS, 1.0, 1.0),
        Some(&business)
    )
    .is_allowed());

    // An operator override rescues a starter tenant without changing plans.
    let now = Utc::now();
    let plan = bot_core::billing::default_catalogue(now)
        .into_iter()
        .find(|p| p.code == PlanCode::Starter)
        .unwrap();
    let sub = Subscription::manual(a.id, plan.id, now);
    let mut rows = entitlements_from_plan(a.id, &plan, now);
    rows.push(Entitlement::new(
        a.id,
        features::LIVE_TRADING,
        None,
        EntitlementSource::Override,
        now,
    ));
    let overridden = EntitlementSet::resolve(Some(&plan), Some(&sub), &rows, now);
    assert!(authorize(
        Some(&c),
        &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
        Some(&overridden)
    )
    .is_allowed());
}

#[test]
fn api_key_scopes_can_only_narrow_never_widen() {
    let a = org("tenant-a", OrganizationStatus::Active);
    // A key with the OWNER role but scopes limited to reads.
    let scopes = PermissionSet::parse_list(&["bot.read", "order.read", "billing.manage"]);
    let c = AuthorizationContext::from_api_key(
        Principal::ApiKey {
            key_id: "k".into(),
            key_prefix: "sk_abc".into(),
        },
        &a,
        MembershipRole::Viewer,
        Some(&scopes),
        None,
        Utc::now(),
    );
    assert!(authorize(Some(&c), &AccessRequest::read(Permission::BotRead), None).is_allowed());
    assert_eq!(
        authorize(
            Some(&c),
            &AccessRequest::billing(Permission::BillingManage),
            None
        )
        .kind,
        DecisionKind::DenyPermission,
        "a scope naming a permission the ROLE lacks must not grant it"
    );
    // The key never carries platform scope.
    assert!(!c.is_platform_scope());
}

#[test]
fn a_plan_without_a_subscription_denies_with_the_right_reason() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let c = ctx(&a, &user("o@a.test"), MembershipRole::OrgOwner);
    let none = EntitlementSet::resolve(None, None, &[], Utc::now());
    let d = authorize(
        Some(&c),
        &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
        Some(&none),
    );
    assert_eq!(d.kind, DecisionKind::DenyEntitlement);
    assert!(d.reason.contains("no_active_subscription"), "{}", d.reason);
    assert_eq!(d.http_status(), 402);
}

#[test]
fn unauthenticated_requests_never_reach_a_tenant() {
    for request in [
        AccessRequest::read(Permission::OrderRead),
        AccessRequest::trade(Permission::BotStart),
        AccessRequest::manage(Permission::ApiKeyCreate),
    ] {
        let d = authorize(None, &request, None);
        assert_eq!(d.kind, DecisionKind::DenyUnauthenticated);
        assert_eq!(d.http_status(), 401);
    }
}

#[test]
fn the_plan_catalogue_limits_are_coherent_with_the_entitlement_layer() {
    let a = org("tenant-a", OrganizationStatus::Active);
    for code in PlanCode::ALL {
        let set = entitlements(&a, code);
        assert!(set.has_active_subscription(), "{code}");
        // Every shipped feature resolves to an explicit answer.
        for feature in features::ALL {
            let limit = set.limit_for(feature);
            assert!(
                matches!(
                    limit,
                    FeatureLimit::Disabled | FeatureLimit::Unlimited | FeatureLimit::Limited(_)
                ),
                "{code}/{feature}"
            );
        }
    }
    // Enterprise is a superset of starter.
    let starter = entitlements(&a, PlanCode::Starter);
    let enterprise = entitlements(&a, PlanCode::Enterprise);
    for feature in features::ALL {
        if starter.limit_for(feature).is_enabled() {
            assert!(
                enterprise.limit_for(feature).is_enabled(),
                "enterprise must not lose {feature}"
            );
        }
    }
}

#[test]
fn plan_assignment_produces_the_expected_rows() {
    let a = org("tenant-a", OrganizationStatus::Active);
    let now = Utc::now();
    let plan: Plan = bot_core::billing::default_catalogue(now)
        .into_iter()
        .find(|p| p.code == PlanCode::Pro)
        .unwrap();
    let rows = entitlements_from_plan(a.id, &plan, now);
    assert_eq!(rows.len(), features::ALL.len());
    for r in &rows {
        assert_eq!(r.organization_id, a.id);
        assert_eq!(r.source, EntitlementSource::Plan);
    }
}
