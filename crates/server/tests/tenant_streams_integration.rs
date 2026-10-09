//! STEP 3 integration: the tenant stream layer — strict tenant scoping
//! end to end.

use bot_core::tenant::OrganizationId;
use sniper_suite::tenant_streams::events::TenantEvent;
use sniper_suite::tenant_streams::filter::TenantStreamFilter;
use sniper_suite::tenant_streams::hub::TenantStreamHub;

fn decision(org: OrganizationId, module: &'static str) -> TenantEvent {
    TenantEvent::Decision {
        organization_id: org,
        decision: "allow",
        module,
        mode: "paper",
        origin: "req",
    }
}

#[tokio::test]
async fn a_subscriber_receives_only_its_own_tenants_events() {
    let hub = TenantStreamHub::new();
    let mine = OrganizationId::new();
    let theirs = OrganizationId::new();

    let mut rx_mine = hub.subscribe(mine).await;
    let mut rx_theirs = hub.subscribe(theirs).await;

    hub.publish(decision(mine, "copy")).await;
    hub.publish(decision(theirs, "sniper")).await;

    let got = rx_mine.recv().await.unwrap();
    assert_eq!(got.organization_id(), mine);
    let got = rx_theirs.recv().await.unwrap();
    assert_eq!(got.organization_id(), theirs);

    // Nothing else was delivered to either.
    assert!(rx_mine.try_recv().is_err());
    assert!(rx_theirs.try_recv().is_err());
}

#[tokio::test]
async fn the_filter_is_a_second_layer_of_scoping() {
    let mine = OrganizationId::new();
    let theirs = OrganizationId::new();
    let filter = TenantStreamFilter::everything_for(mine);

    // Cross-tenant events are rejected even if handed directly.
    assert!(!filter.accepts(&decision(theirs, "copy")));
    // Refinements only narrow.
    let narrow = filter
        .clone()
        .with_kinds(vec!["decision"])
        .with_modules(vec!["copy"]);
    assert!(narrow.accepts(&decision(mine, "copy")));
    assert!(!narrow.accepts(&decision(mine, "sniper")));
    assert!(!filter
        .clone()
        .with_kinds(vec!["runtime_changed"])
        .accepts(&decision(mine, "copy")));
}

#[tokio::test]
async fn publishing_with_no_subscribers_is_a_no_op() {
    let hub = TenantStreamHub::new();
    let org = OrganizationId::new();
    assert_eq!(hub.publish(decision(org, "copy")).await, 0);
    // Publishing self-reaps: nothing is left behind for reap_empty.
    assert_eq!(hub.reap_empty().await, 0);
    assert_eq!(hub.channel_count().await, 0);
}
