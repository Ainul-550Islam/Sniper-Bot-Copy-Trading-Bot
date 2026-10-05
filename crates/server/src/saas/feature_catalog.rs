//! Product feature entitlements catalog and quota definitions (THIRD.md §140).

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
            key: "bot.sniper".into(),
            name: "Solana DEX Sniper".into(),
            description: "Block-0 token launch sniper with Jito bundle relay".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "bot.copy".into(),
            name: "Copy Trading Engine".into(),
            description: "Autonomous high-winrate wallet mirroring".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "bot.polymarket".into(),
            name: "Polymarket CLOB".into(),
            description: "Binary event outcome trading and spread capture".into(),
            category: "Trading".into(),
        },
        FeatureEntitlement {
            key: "custody.kms".into(),
            name: "FIPS 140-3 AWS KMS Custody".into(),
            description: "Hardware signer profile and key envelope encryption".into(),
            category: "Security".into(),
        },
        FeatureEntitlement {
            key: "backtest.engine".into(),
            name: "Deterministic Backtester".into(),
            description: "Historical simulation with exact fee & slippage modeling".into(),
            category: "Analytics".into(),
        },
        FeatureEntitlement {
            key: "reports.audit".into(),
            name: "Compliance Audit Exporter".into(),
            description: "SOC2 and Capital Gains ledger exports in CSV/JSON/PDF".into(),
            category: "Compliance".into(),
        },
    ]
}
