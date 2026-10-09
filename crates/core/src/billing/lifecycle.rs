//! Authoritative billing subscription and entitlement lifecycle transitions (FIFTH.md §223).

use serde::{Deserialize, Serialize};

use crate::billing::subscription::SubscriptionStatus;

/// Lifecycle transitions permissible for tenant subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionTransition {
    Activate,
    EnterPastDue,
    Suspend,
    Cancel,
    Expire,
    Resume,
}

/// Evaluates whether a requested transition is valid from the current state.
pub fn is_valid_transition(
    current: SubscriptionStatus,
    transition: SubscriptionTransition,
) -> bool {
    match (current, transition) {
        (SubscriptionStatus::Trialing, SubscriptionTransition::Activate) => true,
        (SubscriptionStatus::Trialing, SubscriptionTransition::Expire) => true,
        (SubscriptionStatus::Trialing, SubscriptionTransition::Cancel) => true,
        (SubscriptionStatus::Active, SubscriptionTransition::EnterPastDue) => true,
        (SubscriptionStatus::Active, SubscriptionTransition::Suspend) => true,
        (SubscriptionStatus::Active, SubscriptionTransition::Cancel) => true,
        (SubscriptionStatus::PastDue, SubscriptionTransition::Activate) => true,
        (SubscriptionStatus::PastDue, SubscriptionTransition::Suspend) => true,
        (SubscriptionStatus::PastDue, SubscriptionTransition::Cancel) => true,
        (SubscriptionStatus::Paused, SubscriptionTransition::Resume) => true,
        (SubscriptionStatus::Paused, SubscriptionTransition::Cancel) => true,
        _ => false,
    }
}

/// Applies the transition and returns the new status or error.
pub fn apply_transition(
    current: SubscriptionStatus,
    transition: SubscriptionTransition,
) -> Result<SubscriptionStatus, &'static str> {
    if !is_valid_transition(current, transition) {
        return Err("illegal_subscription_state_transition");
    }

    Ok(match transition {
        SubscriptionTransition::Activate | SubscriptionTransition::Resume => {
            SubscriptionStatus::Active
        }
        SubscriptionTransition::EnterPastDue => SubscriptionStatus::PastDue,
        SubscriptionTransition::Suspend => SubscriptionStatus::Paused,
        SubscriptionTransition::Cancel => SubscriptionStatus::Canceled,
        SubscriptionTransition::Expire => SubscriptionStatus::Expired,
    })
}
