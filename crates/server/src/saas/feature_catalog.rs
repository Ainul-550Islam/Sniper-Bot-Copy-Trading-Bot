//! Shared feature vocabulary exposed by the control plane.
//!
//! This catalogue describes capability keys only. Whether a tenant may use a
//! capability is determined by its durable plan and entitlement rows.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEntitlement {
    pub key: String,
    pub name: String,
    pub description: String,
    pub category: String,
}

pub fn all_features() -> Vec<FeatureEntitlement> {
    vec![
        FeatureEntitlement {
            key: "module.sniper".into(),
            name: "Sniper module".into(),
            description: "Tenant entitlement key for the sniper module.".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "module.copy".into(),
            name: "Copy module".into(),
            description: "Tenant entitlement key for the copy module.".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "module.polymarket".into(),
            name: "Polymarket module".into(),
            description: "Tenant entitlement key for the Polymarket module.".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "feature.live_trading".into(),
            name: "Live trading".into(),
            description: "Tenant entitlement key for live trading activity.".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "feature.api_keys".into(),
            name: "API keys".into(),
            description: "Tenant entitlement key for API-key access.".into(),
            category: "Access".into(),
        },
        FeatureEntitlement {
            key: "feature.exports".into(),
            name: "Data exports".into(),
            description: "Tenant entitlement key for data exports.".into(),
            category: "Reporting".into(),
        },
    ]
}
