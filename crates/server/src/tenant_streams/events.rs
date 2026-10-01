//! The typed tenant event envelope (STEP 3 file 46).

use serde::Serialize;

use bot_core::tenant::OrganizationId;

/// One tenant-scoped event. The organization id is part of EVERY variant
/// payload (never a wrapper field a publisher can forget to set).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TenantEvent {
    /// The gateway made a decision for this tenant.
    Decision {
        /// The tenant.
        organization_id: OrganizationId,
        /// `"allow"` or the deny label (see the gateway's vocabulary).
        decision: &'static str,
        /// The module the request targeted.
        module: &'static str,
        /// The trading mode the request targeted.
        mode: &'static str,
        /// Where the request came from.
        origin: &'static str,
    },
    /// An execution started under an issued context.
    ExecutionStarted {
        /// The tenant.
        organization_id: OrganizationId,
        /// The execution trace's correlation id.
        correlation_id: String,
        /// The module running it.
        module: &'static str,
        /// The trading mode.
        mode: &'static str,
    },
    /// An execution reached a terminal state.
    ExecutionFinished {
        /// The tenant.
        organization_id: OrganizationId,
        /// The execution trace's correlation id.
        correlation_id: String,
        /// Outcome label (confirmed/failed/expired).
        outcome: &'static str,
    },
    /// The tenant's runtime registry changed (registered/rotated/
    /// drained/stopped/reaped).
    RuntimeChanged {
        /// The tenant.
        organization_id: OrganizationId,
        /// What happened.
        change: &'static str,
        /// The runtime generation after the change.
        generation: u64,
    },
    /// The tenant's configuration changed.
    ConfigChanged {
        /// The tenant.
        organization_id: OrganizationId,
        /// The new document version.
        version: u64,
        /// How many discrete changes the diff found.
        changes: usize,
    },
}

impl TenantEvent {
    /// The tenant this event belongs to (the mandatory scope).
    pub fn organization_id(&self) -> OrganizationId {
        match self {
            TenantEvent::Decision {
                organization_id, ..
            }
            | TenantEvent::ExecutionStarted {
                organization_id, ..
            }
            | TenantEvent::ExecutionFinished {
                organization_id, ..
            }
            | TenantEvent::RuntimeChanged {
                organization_id, ..
            }
            | TenantEvent::ConfigChanged {
                organization_id, ..
            } => *organization_id,
        }
    }

    /// Stable event kind label (consumers switch on it).
    pub fn kind(&self) -> &'static str {
        match self {
            TenantEvent::Decision { .. } => "decision",
            TenantEvent::ExecutionStarted { .. } => "execution_started",
            TenantEvent::ExecutionFinished { .. } => "execution_finished",
            TenantEvent::RuntimeChanged { .. } => "runtime_changed",
            TenantEvent::ConfigChanged { .. } => "config_changed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_carries_its_organization() {
        let org = OrganizationId::new();
        let events = vec![
            TenantEvent::Decision {
                organization_id: org,
                decision: "allow",
                module: "copy",
                mode: "paper",
                origin: "req",
            },
            TenantEvent::ExecutionStarted {
                organization_id: org,
                correlation_id: "c-1".into(),
                module: "copy",
                mode: "paper",
            },
            TenantEvent::ExecutionFinished {
                organization_id: org,
                correlation_id: "c-1".into(),
                outcome: "confirmed",
            },
            TenantEvent::RuntimeChanged {
                organization_id: org,
                change: "rotated",
                generation: 2,
            },
            TenantEvent::ConfigChanged {
                organization_id: org,
                version: 3,
                changes: 2,
            },
        ];
        for event in &events {
            assert_eq!(event.organization_id(), org);
            assert!(!event.kind().is_empty());
            // Serializable for the websocket layer.
            assert!(serde_json::to_value(event).is_ok());
        }
    }
}
