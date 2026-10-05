//! Solana token safety, honeypot detection, and holder concentration analysis.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Detailed token safety audit result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSafetyReport {
    pub mint: String,
    pub is_mint_authority_renounced: bool,
    pub is_freeze_authority_disabled: bool,
    pub is_lp_locked_or_burned: bool,
    pub top_10_holder_percent: f64,
    pub bundler_detected: bool,
    pub safety_score: u32,
    pub verdict: SafetyVerdict,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyVerdict {
    Safe,
    Caution,
    HighRiskHoneypot,
}

impl SafetyVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Safe => "SAFE",
            Self::Caution => "CAUTION",
            Self::HighRiskHoneypot => "HIGH_RISK_HONEYPOT",
        }
    }
}

pub struct TokenSafetyAuditor;

impl TokenSafetyAuditor {
    /// Analyzes token on-chain parameters for rug-pull and honeypot indicators.
    pub fn audit_token(
        mint: &Pubkey,
        mint_authority: Option<&Pubkey>,
        freeze_authority: Option<&Pubkey>,
        holder_percentages: &[f64],
        lp_burn_pct: f64,
        genesis_slot_trades: usize,
    ) -> TokenSafetyReport {
        let mut flags = Vec::new();
        let mut score: u32 = 100;

        // 1. Mint Authority Check
        let is_mint_authority_renounced = mint_authority.is_none();
        if !is_mint_authority_renounced {
            score = score.saturating_sub(35);
            flags.push("MINT_AUTHORITY_ACTIVE: Creator can mint infinite tokens".into());
        }

        // 2. Freeze Authority Check
        let is_freeze_authority_disabled = freeze_authority.is_none();
        if !is_freeze_authority_disabled {
            score = score.saturating_sub(45);
            flags.push("FREEZE_AUTHORITY_ACTIVE: Creator can freeze user token accounts (Honeypot Risk)".into());
        }

        // 3. Holder Concentration Check
        let top_10_pct: f64 = holder_percentages.iter().take(10).sum();
        if top_10_pct > 30.0 {
            score = score.saturating_sub(25);
            flags.push(format!("HIGH_HOLDER_CONCENTRATION: Top 10 hold {:.2}%", top_10_pct));
        }

        // 4. LP Lock / Burn Verification
        let is_lp_locked_or_burned = lp_burn_pct >= 95.0;
        if !is_lp_locked_or_burned {
            score = score.saturating_sub(30);
            flags.push(format!("LOW_LP_BURN: Only {:.1}% LP locked/burned", lp_burn_pct));
        }

        // 5. Bundler Detection
        let bundler_detected = genesis_slot_trades > 4;
        if bundler_detected {
            score = score.saturating_sub(15);
            flags.push(format!("BUNDLER_ACTIVITY_DETECTED: {} genesis slot snipes", genesis_slot_trades));
        }

        let verdict = if score >= 80 && is_freeze_authority_disabled {
            SafetyVerdict::Safe
        } else if score >= 50 {
            SafetyVerdict::Caution
        } else {
            SafetyVerdict::HighRiskHoneypot
        };

        TokenSafetyReport {
            mint: mint.to_string(),
            is_mint_authority_renounced,
            is_freeze_authority_disabled,
            is_lp_locked_or_burned,
            top_10_holder_percent: top_10_pct,
            bundler_detected,
            safety_score: score,
            verdict,
            flags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_token_scores_safe() {
        let mint = Pubkey::new_unique();
        let holders = vec![4.0, 3.5, 3.0, 2.5, 2.0, 1.5, 1.0, 1.0, 0.8, 0.7]; // Top 10 = 20%
        let report = TokenSafetyAuditor::audit_token(&mint, None, None, &holders, 100.0, 1);
        assert_eq!(report.verdict, SafetyVerdict::Safe);
        assert!(report.safety_score >= 80);
        assert!(report.flags.is_empty());
    }

    #[test]
    fn freeze_authority_triggers_honeypot_warning() {
        let mint = Pubkey::new_unique();
        let evil_auth = Pubkey::new_unique();
        let holders = vec![5.0; 10];
        let report = TokenSafetyAuditor::audit_token(&mint, None, Some(&evil_auth), &holders, 100.0, 1);
        assert_eq!(report.verdict, SafetyVerdict::Caution);
        assert!(report.flags.iter().any(|f| f.contains("FREEZE_AUTHORITY_ACTIVE")));
    }
}
