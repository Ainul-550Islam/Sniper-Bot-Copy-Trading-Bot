//! WebSocket / stream cross-tenant isolation tests (tenant-isolation
//! file 71).
//!
//! Positive: a subscription scope receives its own tenant's events
//! through the hub; refinements (kinds/modules) only narrow.
//!
//! Negative: an event for tenant B is NEVER delivered to tenant A's
//! subscription (filter AND scope both enforce it); there is no
//! global scope to subscribe with; a subscription cannot be widened
//! after construction.

use sniper_suite::tenant_streams::events::TenantEvent;
use sniper_suite::tenant_streams::filter::TenantStreamFilter;
use sniper_suite::tenant_streams::hub::TenantStreamHub;
use sniper_suite::tenant_streams::subscription_scope::SubscriptionScope;

use bot_core::tenant::OrganizationId;

fn decision(organization_id: OrganizationId) -> TenantEvent {
    TenantEvent::Decision {
        organization_id,
        decision: "allow",
        module: "copy",
        mode: "paper",
        origin: "http",
    }
}

fn execution_started(organization_id: OrganizationId) -> TenantEvent {
    TenantEvent::ExecutionStarted {
        organization_id,
        correlation_id: "corr-1".into(),
        module: "copy",
        mode: "paper",
    }
}

#[tokio::test]
async fn a_tenant_receives_only_its_own_events_through_the_hub() {
    let hub = TenantStreamHub::new();
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    let mut a_receiver = hub.subscribe(a).await;
    let mut b_receiver = hub.subscribe(b).await;

    // Publish one event per tenant.
    let delivered = hub.publish(decision(a)).await;
    assert_eq!(delivered, 1);
    let delivered = hub.publish(decision(b)).await;
    assert_eq!(delivered, 1);

    // Each receiver sees ONLY its own tenant's event.
    let seen_a = a_receiver.recv().await.unwrap();
    assert_eq!(seen_a.organization_id(), a);
    let seen_b = b_receiver.recv().await.unwrap();
    assert_eq!(seen_b.organization_id(), b);

    // An event for a tenant with NO subscribers is dropped, not
    // fanned out globally (delivered = 0).
    let c = OrganizationId::new();
    assert_eq!(hub.publish(decision(c)).await, 0);
    assert_eq!(hub.channel_count().await, 3);
}

#[tokio::test]
async fn a_cross_tenant_event_is_never_delivered_to_a_subscriber() {
    let hub = TenantStreamHub::new();
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    let mut a_receiver = hub.subscribe(a).await;
    // Publish B's event: A's receiver must never see it (it is not on
    // B's channel at all).
    hub.publish(execution_started(b)).await;
    // Publish A's event so we can assert the receiver's NEXT event is
    // A's, proving B's was skipped rather than buffered.
    hub.publish(decision(a)).await;
    let seen = a_receiver.recv().await.unwrap();
    assert_eq!(seen.organization_id(), a);
    assert_eq!(seen.kind(), "decision");
    // Nothing else pending for A.
    assert!(a_receiver.try_recv().is_err());
}

#[test]
fn the_filter_checks_the_organization_before_anything() {
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let filter = TenantStreamFilter::everything_for(a);
    assert!(filter.accepts(&decision(a)));
    assert!(!filter.accepts(&decision(b)));
    // Refinements never widen the scope.
    let narrowed = filter.with_kinds(vec!["decision"]);
    assert!(narrowed.accepts(&decision(a)));
    assert!(!narrowed.accepts(&execution_started(a)));
    assert!(!narrowed.accepts(&execution_started(b)));
}

#[test]
fn a_subscription_scope_receives_only_its_own_tenants_events() {
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let scope = SubscriptionScope::new(a, "sub-1");
    assert!(scope.accepts(&decision(a)));
    assert!(!scope.accepts(&decision(b)));
    assert!(scope.belongs_to(a));
    assert!(!scope.belongs_to(b));
    assert_eq!(scope.subscription_id(), "sub-1");
    assert_eq!(scope.filter().organization_id, a);
}

#[test]
fn scope_refinements_only_narrow_never_widen() {
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let scope = SubscriptionScope::new(a, "sub-1").with_kinds(vec!["decision"]);
    // Own-tenant matching-kind event passes.
    assert!(scope.accepts(&decision(a)));
    // Non-matching kind is dropped for the OWN tenant.
    assert!(!scope.accepts(&execution_started(a)));
    // And no refinement admits another tenant.
    assert!(!scope.accepts(&decision(b)));
    assert!(!scope.accepts(&execution_started(b)));
}

#[tokio::test]
async fn an_empty_hub_reaps_nothing_and_a_drained_channel_disappears() {
    let hub = TenantStreamHub::new();
    assert_eq!(hub.reap_empty().await, 0);
    let a = OrganizationId::new();
    let receiver = hub.subscribe(a).await;
    assert_eq!(hub.receiver_count(a).await, 1);
    drop(receiver);
    // The channel has no receivers now: reaping drops it so the map
    // stays bounded by ACTIVE subscriptions.
    let reaped = hub.reap_empty().await;
    assert_eq!(reaped, 1);
    assert_eq!(hub.channel_count().await, 0);
}
