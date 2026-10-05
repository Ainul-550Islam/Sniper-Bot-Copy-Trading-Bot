//! Durable domain event schema for billing lifecycle changes (FIFTH.md §221).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::billing::plan::PlanCode;
use crate::billing::subscription::{BillingProvider, SubscriptionId, SubscriptionStatus};
use crate::tenant::OrganizationId;

/// Unique billing event identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BillingEventId(pub uuid::Uuid);

impl BillingEventId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// Typed domain events emitted on billing state mutations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BillingEvent {
    SubscriptionCreated {
        id: BillingEventId,
        organization_id: OrganizationId,
        subscription_id: SubscriptionId,
        plan_code: PlanCode,
        provider: BillingProvider,
        status: SubscriptionStatus,
        occurred_at: DateTime<Utc>,
    },
    PlanChanged {
        id: BillingEventId,
        organization_id: OrganizationId,
        subscription_id: SubscriptionId,
        old_plan: PlanCode,
        new_plan: PlanCode,
        occurred_at: DateTime<Utc>,
    },
    InvoiceSettled {
        id: BillingEventId,
        organization_id: OrganizationId,
        invoice_id: String,
        amount_usd_cents: u64,
        occurred_at: DateTime<Utc>,
    },
    PaymentFailed {
        id: BillingEventId,
        organization_id: OrganizationId,
        invoice_id: String,
        reason: String,
        occurred_at: DateTime<Utc>,
    },
    DunningStateChanged {
        id: BillingEventId,
        organization_id: OrganizationId,
        dunning_state: String,
        occurred_at: DateTime<Utc>,
    },
}

impl BillingEvent {
    pub fn organization_id(&self) -> OrganizationId {
        match self {
            Self::SubscriptionCreated {
                organization_id, ..
            } => *organization_id,
            Self::PlanChanged {
                organization_id, ..
            } => *organization_id,
            Self::InvoiceSettled {
                organization_id, ..
            } => *organization_id,
            Self::PaymentFailed {
                organization_id, ..
            } => *organization_id,
            Self::DunningStateChanged {
                organization_id, ..
            } => *organization_id,
        }
    }

    pub fn occurred_at(&self) -> DateTime<Utc> {
        match self {
            Self::SubscriptionCreated { occurred_at, .. } => *occurred_at,
            Self::PlanChanged { occurred_at, .. } => *occurred_at,
            Self::InvoiceSettled { occurred_at, .. } => *occurred_at,
            Self::PaymentFailed { occurred_at, .. } => *occurred_at,
            Self::DunningStateChanged { occurred_at, .. } => *occurred_at,
        }
    }
}
