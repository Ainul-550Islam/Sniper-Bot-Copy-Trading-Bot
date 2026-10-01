//! The per-tenant stream hub (STEP 3 file 47).
//!
//! One bounded broadcast channel per organization, created on first
//! publish or subscribe and dropped when its last receiver goes away
//! (the hub reaps empty channels opportunistically on publish/subscribe).
//! Publishing is non-blocking: a full channel drops the OLDEST buffered
//! event for the slowest receiver — streaming is observability, never a
//! path that may delay execution.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use bot_core::tenant::OrganizationId;

use super::events::TenantEvent;

/// Per-tenant channel capacity.
const CHANNEL_CAPACITY: usize = 256;

/// The hub.
#[derive(Default)]
pub struct TenantStreamHub {
    channels: RwLock<HashMap<OrganizationId, broadcast::Sender<TenantEvent>>>,
}

impl TenantStreamHub {
    /// An empty hub.
    pub fn new() -> Self {
        TenantStreamHub::default()
    }

    /// Publish an event to the tenant's channel. Returns the number of
    /// receivers it was delivered to (0 = nobody listening — the event
    /// is intentionally dropped, not queued globally).
    pub async fn publish(&self, event: TenantEvent) -> usize {
        let org = event.organization_id();
        let sender = self.channel_for(org).await;
        sender.send(event).unwrap_or(0)
    }

    /// Subscribe to a tenant's events.
    pub async fn subscribe(
        &self,
        organization_id: OrganizationId,
    ) -> broadcast::Receiver<TenantEvent> {
        self.channel_for(organization_id).await.subscribe()
    }

    /// Drop channels with no receivers (called opportunistically; keeps
    /// the map bounded by ACTIVE subscriptions, not by history).
    pub async fn reap_empty(&self) -> usize {
        let mut channels = self.channels.write().await;
        let before = channels.len();
        channels.retain(|_, sender| sender.receiver_count() > 0);
        before - channels.len()
    }

    /// The number of live channels (observability, tests).
    pub async fn channel_count(&self) -> usize {
        self.channels.read().await.len()
    }

    /// The number of receivers for one tenant (observability, tests).
    pub async fn receiver_count(&self, organization_id: OrganizationId) -> usize {
        self.channels
            .read()
            .await
            .get(&organization_id)
            .map(|s| s.receiver_count())
            .unwrap_or(0)
    }

    async fn channel_for(&self, organization_id: OrganizationId) -> broadcast::Sender<TenantEvent> {
        let mut channels = self.channels.write().await;
        channels
            .entry(organization_id)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}

/// Share the hub.
pub fn shared() -> Arc<TenantStreamHub> {
    Arc::new(TenantStreamHub::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(org: OrganizationId) -> TenantEvent {
        TenantEvent::Decision {
            organization_id: org,
            decision: "allow",
            module: "copy",
            mode: "paper",
            origin: "req",
        }
    }

    #[tokio::test]
    async fn subscribers_receive_only_their_tenants_events() {
        let hub = TenantStreamHub::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let mut rx_a = hub.subscribe(a).await;
        let mut rx_b = hub.subscribe(b).await;

        hub.publish(decision(a)).await;

        let received = rx_a.recv().await.unwrap();
        assert_eq!(received.organization_id(), a);
        // Tenant B's channel got nothing.
        assert!(rx_b.try_recv().is_err());
    }

    #[tokio::test]
    async fn publishing_without_subscribers_drops_and_moves_on() {
        let hub = TenantStreamHub::new();
        let org = OrganizationId::new();
        assert_eq!(hub.publish(decision(org)).await, 0);
    }

    #[tokio::test]
    async fn empty_channels_are_reaped() {
        let hub = TenantStreamHub::new();
        let org = OrganizationId::new();
        hub.publish(decision(org)).await; // creates the channel
        assert_eq!(hub.channel_count().await, 1);

        assert_eq!(hub.reap_empty().await, 1);
        assert_eq!(hub.channel_count().await, 0);
    }

    #[tokio::test]
    async fn active_channels_survive_the_reap() {
        let hub = TenantStreamHub::new();
        let org = OrganizationId::new();
        let _rx = hub.subscribe(org).await;
        hub.publish(decision(org)).await;
        assert_eq!(hub.reap_empty().await, 0);
        assert_eq!(hub.receiver_count(org).await, 1);
    }
}
