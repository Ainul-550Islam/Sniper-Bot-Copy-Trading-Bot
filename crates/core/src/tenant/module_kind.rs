//! Typed module identity for tenancy (STEP 3 file 05).
//!
//! The canonical module enum is [`crate::models::BotModule`] — this module
//! does NOT duplicate it. [`ModuleKind`] is a re-export alias plus the
//! tenancy mappings the guard layer needs: which plan feature key gates
//! each module, which modules can move money, and the string forms used
//! by entitlement rows, wallet bindings and configuration.

use crate::billing::features;
use crate::models::BotModule;

/// The canonical module identity (re-exported under its tenancy name).
pub type ModuleKind = BotModule;

/// The modules that can place orders and therefore participate in the
/// execution authorization chain.
pub const TRADING_MODULES: [ModuleKind; 3] = BotModule::TRADING;

/// Every module, stable order.
pub const ALL_MODULES: [ModuleKind; 5] = BotModule::ALL;

/// The plan feature key that gates this module, if any.
///
/// * `Sniper` / `Copy` / `Polymarket` are plan-gated trading modules
///   (`module.sniper`, `module.copy`, `module.polymarket`).
/// * `Telegram` is the control plane — an authenticated tenant always has
///   it; it never trades by itself.
/// * `Contract` (on-chain staking suite) is not a SaaS-plan feature at
///   this stage of the program.
pub fn feature_key(module: ModuleKind) -> Option<&'static str> {
    match module {
        ModuleKind::Sniper => Some(features::MODULE_SNIPER),
        ModuleKind::Copy => Some(features::MODULE_COPY),
        ModuleKind::Polymarket => Some(features::MODULE_POLYMARKET),
        ModuleKind::Telegram | ModuleKind::Contract => None,
    }
}

/// Parse a module from its stable string form (`"sniper"`, `"copy"`, …).
pub fn parse_module(s: &str) -> Option<ModuleKind> {
    s.trim().parse().ok()
}

/// Is this module one that can move money (and therefore requires the
/// full wallet/signer/risk guard chain)?
pub fn can_trade(module: ModuleKind) -> bool {
    TRADING_MODULES.contains(&module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_keys_match_the_billing_vocabulary() {
        assert_eq!(feature_key(ModuleKind::Sniper), Some("module.sniper"));
        assert_eq!(feature_key(ModuleKind::Copy), Some("module.copy"));
        assert_eq!(
            feature_key(ModuleKind::Polymarket),
            Some("module.polymarket")
        );
        assert_eq!(feature_key(ModuleKind::Telegram), None);
        assert_eq!(feature_key(ModuleKind::Contract), None);
    }

    #[test]
    fn parse_round_trips_the_stable_labels() {
        for m in ALL_MODULES {
            assert_eq!(parse_module(m.as_str()), Some(m));
        }
        assert_eq!(parse_module("nope"), None);
        // The existing BotModule FromStr is case-insensitive and knows
        // the operator aliases — the tenancy view inherits both.
        assert_eq!(parse_module(" SNIPER "), Some(ModuleKind::Sniper));
        assert_eq!(parse_module("poly"), Some(ModuleKind::Polymarket));
    }

    #[test]
    fn trading_modules_are_exactly_the_money_movers() {
        assert!(can_trade(ModuleKind::Sniper));
        assert!(can_trade(ModuleKind::Copy));
        assert!(can_trade(ModuleKind::Polymarket));
        assert!(!can_trade(ModuleKind::Telegram));
        assert!(!can_trade(ModuleKind::Contract));
        // The trading set and the plan-gated set agree.
        for m in TRADING_MODULES {
            assert!(feature_key(m).is_some());
        }
    }
}
