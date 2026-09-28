//! Explicit funded-live mode guard (Batch 7).
//! Detect: simulate, paper, dry-run, live-unfunded, live-funded.
//! Live-funded requires explicit configuration and operator authorization.
//! Never enable funded mode by default. Never expose wallet private keys.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FundedMode {
    Simulate,
    Paper,
    DryRun,
    LiveUnfunded,
    LiveFunded,
}

impl FundedMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Simulate => "simulate",
            Self::Paper => "paper",
            Self::DryRun => "dry_run",
            Self::LiveUnfunded => "live_unfunded",
            Self::LiveFunded => "live_funded",
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::LiveFunded | Self::LiveUnfunded)
    }

    pub fn is_funded(&self) -> bool {
        matches!(self, Self::LiveFunded)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundedModeConfig {
    pub execution_mode: String,
    pub allow_live_trading: bool,
    pub wallet_configured: bool,
    pub wallet_funded: bool,
    pub owner_authorized: bool,
    pub risk_gates_pass: bool,
}

impl FundedModeConfig {
    pub fn detect_mode(&self) -> FundedMode {
        let mode_lower = self.execution_mode.to_lowercase();
        if mode_lower == "simulate" {
            return FundedMode::Simulate;
        }
        if mode_lower == "paper" {
            return FundedMode::Paper;
        }
        if mode_lower == "dry_run" || mode_lower == "dry-run" {
            return FundedMode::DryRun;
        }
        if mode_lower == "live" {
            if self.allow_live_trading
                && self.wallet_configured
                && self.wallet_funded
                && self.owner_authorized
                && self.risk_gates_pass
            {
                return FundedMode::LiveFunded;
            } else {
                return FundedMode::LiveUnfunded;
            }
        }
        // Default safe
        FundedMode::Paper
    }

    pub fn is_live_funded_allowed(&self) -> bool {
        self.detect_mode() == FundedMode::LiveFunded
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "execution_mode": self.execution_mode,
            "allow_live_trading": self.allow_live_trading,
            "wallet_configured": self.wallet_configured,
            "wallet_funded": self.wallet_funded,
            "owner_authorized": self.owner_authorized,
            "risk_gates_pass": self.risk_gates_pass,
            "detected_mode": self.detect_mode().as_str(),
        })
    }
}

pub struct FundedModeGuard;

impl FundedModeGuard {
    pub fn check(config: &FundedModeConfig) -> Result<FundedMode, String> {
        let mode = config.detect_mode();
        match mode {
            FundedMode::Simulate | FundedMode::Paper | FundedMode::DryRun => Ok(mode),
            FundedMode::LiveUnfunded => Err("live mode requested but wallet not funded or not authorized or risk gates fail — live-funded guard denies".into()),
            FundedMode::LiveFunded => {
                // All gates must pass — already ensured by detect_mode, but double-check
                if !config.allow_live_trading {
                    return Err("allow_live_trading=false — funded mode denied".into());
                }
                if !config.wallet_configured {
                    return Err("wallet not configured — funded mode denied".into());
                }
                if !config.wallet_funded {
                    return Err("wallet not funded — funded mode denied".into());
                }
                if !config.owner_authorized {
                    return Err("owner not authorized — funded mode denied".into());
                }
                if !config.risk_gates_pass {
                    return Err("risk gates fail — funded mode denied".into());
                }
                Ok(mode)
            }
        }
    }

    pub fn never_expose_private_key(key: &str) -> String {
        if key.trim().is_empty() {
            "<not configured>".into()
        } else {
            "<redacted>".into()
        }
    }

    pub fn default_is_not_funded() -> bool {
        let cfg = FundedModeConfig {
            execution_mode: "paper".into(),
            allow_live_trading: false,
            wallet_configured: false,
            wallet_funded: false,
            owner_authorized: false,
            risk_gates_pass: false,
        };
        cfg.detect_mode() != FundedMode::LiveFunded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_not_funded() {
        assert!(FundedModeGuard::default_is_not_funded());
        let cfg = FundedModeConfig {
            execution_mode: "paper".into(),
            allow_live_trading: false,
            wallet_configured: false,
            wallet_funded: false,
            owner_authorized: false,
            risk_gates_pass: false,
        };
        assert_eq!(cfg.detect_mode(), FundedMode::Paper);
        assert!(!cfg.is_live_funded_allowed());
    }

    #[test]
    fn dry_run_is_not_funded() {
        let cfg = FundedModeConfig {
            execution_mode: "dry_run".into(),
            allow_live_trading: false,
            wallet_configured: true,
            wallet_funded: true,
            owner_authorized: true,
            risk_gates_pass: true,
        };
        assert_eq!(cfg.detect_mode(), FundedMode::DryRun);
        assert!(!cfg.is_live_funded_allowed());
    }

    #[test]
    fn live_unfunded_denied() {
        let cfg = FundedModeConfig {
            execution_mode: "live".into(),
            allow_live_trading: true,
            wallet_configured: true,
            wallet_funded: false,
            owner_authorized: true,
            risk_gates_pass: true,
        };
        assert_eq!(cfg.detect_mode(), FundedMode::LiveUnfunded);
        assert!(FundedModeGuard::check(&cfg).is_err());
    }

    #[test]
    fn live_funded_requires_all_gates() {
        let base = FundedModeConfig {
            execution_mode: "live".into(),
            allow_live_trading: true,
            wallet_configured: true,
            wallet_funded: true,
            owner_authorized: true,
            risk_gates_pass: true,
        };
        assert_eq!(base.detect_mode(), FundedMode::LiveFunded);
        assert!(FundedModeGuard::check(&base).is_ok());

        // Each gate missing should deny
        let mut cfg = base.clone();
        cfg.allow_live_trading = false;
        assert!(FundedModeGuard::check(&cfg).is_err());
        cfg = base.clone();
        cfg.wallet_funded = false;
        assert!(FundedModeGuard::check(&cfg).is_err());
        cfg = base.clone();
        cfg.owner_authorized = false;
        assert!(FundedModeGuard::check(&cfg).is_err());
    }

    #[test]
    fn never_expose_private_keys() {
        assert_eq!(
            FundedModeGuard::never_expose_private_key("secret123"),
            "<redacted>"
        );
        assert_eq!(
            FundedModeGuard::never_expose_private_key(""),
            "<not configured>"
        );
        let cfg = FundedModeConfig {
            execution_mode: "live".into(),
            allow_live_trading: true,
            wallet_configured: true,
            wallet_funded: true,
            owner_authorized: true,
            risk_gates_pass: true,
        };
        let json = cfg.to_safe_json().to_string();
        assert!(!json.contains("secret123"));
    }

    #[test]
    fn simulate_paper_are_safe() {
        for mode in ["simulate", "paper", "dry_run"] {
            let cfg = FundedModeConfig {
                execution_mode: mode.into(),
                allow_live_trading: false,
                wallet_configured: false,
                wallet_funded: false,
                owner_authorized: false,
                risk_gates_pass: false,
            };
            assert!(FundedModeGuard::check(&cfg).is_ok());
            assert!(!cfg.is_live_funded_allowed());
        }
    }
}
