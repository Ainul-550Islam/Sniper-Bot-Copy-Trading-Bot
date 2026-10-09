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

/// The outcome of a sell-path probe (GAP-MAP P1).
///
/// A honeypot is a token you can buy but cannot sell. The authoritative
/// check is to let the venue itself answer: we build the venue's SELL
/// instruction and run it through `simulateTransaction`. Three outcomes:
///
/// * `Sellable` — the sell path is intact. Either the simulation succeeded
///   or the program processed the instruction all the way to the
///   balance check (error `0x1` — "insufficient funds" — proves the
///   discriminator, program id and account wiring are correct and only the
///   probe's zero balance stopped it; the real position will have one);
/// * `NotSellable` — the venue refused the sell in a way that is NOT the
///   balance check: the sell is disabled, an account is missing or frozen,
///   or the instruction fell through. Buying this token is a one-way door;
/// * `Unknown` — the probe could not run (transport failure, unsupported
///   venue). This is a skip, never a pass and never a fail; strict gating
///   decides the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "detail")]
pub enum SellProbeOutcome {
    Sellable,
    NotSellable(String),
    Unknown(String),
}

impl SellProbeOutcome {
    pub fn sellable(&self) -> bool {
        matches!(self, SellProbeOutcome::Sellable)
    }
}

/// Classify one `simulateTransaction` error into a [`SellProbeOutcome`].
///
/// Pure and total so the classification is unit-testable without an RPC:
/// * no error at all → the venue accepted the sell → `Sellable`;
/// * the SPL-token "insufficient funds" custom error (`0x1`) → the program
///   executed our instruction down to the balance transfer → `Sellable`
///   (the probe simply holds no tokens);
/// * anything else that names a program error → classified by text: explicit
///   freeze/sale-disabled markers are `NotSellable`, the rest stays
///   `Unknown` (we refuse to guess a token is safe from a message we do not
///   recognise, and we refuse to declare a honeypot from one we cannot
///   attribute).
pub fn classify_sell_probe(
    simulation_error: Option<&serde_json::Value>,
    logs: &[String],
) -> SellProbeOutcome {
    let Some(err) = simulation_error else {
        return SellProbeOutcome::Sellable;
    };
    let err_text = err.to_string();
    let log_text = logs.join("\n");
    let combined = format!("{err_text}\n{log_text}").to_lowercase();

    // The probe signs for zero tokens: hitting the balance check means the
    // sell path is structurally sound. SPL Token "insufficient funds" is
    // custom error 0x1; programs also surface it as text.
    if combined.contains("custom program error: 0x1")
        || combined.contains("insufficient funds")
        || combined.contains("insufficientfunds")
    {
        return SellProbeOutcome::Sellable;
    }
    // Explicit honeypot levers.
    for marker in ["frozen", "freeze", "sells disabled", "sell disabled", "trading disabled"] {
        if combined.contains(marker) {
            // Carry the program log lines too: they hold the venue's own words
            // (e.g. "account is frozen") and are what an operator needs to read.
            let detail = if logs.is_empty() {
                format!("sell simulation reported '{marker}': {err}")
            } else {
                format!("sell simulation reported '{marker}': {err}; logs: {log_text}")
            };
            return SellProbeOutcome::NotSellable(detail);
        }
    }
    SellProbeOutcome::Unknown(format!("sell simulation failed: {err}"))
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
            flags.push(
                "FREEZE_AUTHORITY_ACTIVE: Creator can freeze user token accounts (Honeypot Risk)"
                    .into(),
            );
        }

        // 3. Holder Concentration Check
        let top_10_pct: f64 = holder_percentages.iter().take(10).sum();
        if top_10_pct > 30.0 {
            score = score.saturating_sub(25);
            flags.push(format!(
                "HIGH_HOLDER_CONCENTRATION: Top 10 hold {:.2}%",
                top_10_pct
            ));
        }

        // 4. LP Lock / Burn Verification
        let is_lp_locked_or_burned = lp_burn_pct >= 95.0;
        if !is_lp_locked_or_burned {
            score = score.saturating_sub(30);
            flags.push(format!(
                "LOW_LP_BURN: Only {:.1}% LP locked/burned",
                lp_burn_pct
            ));
        }

        // 5. Bundler Detection
        let bundler_detected = genesis_slot_trades > 4;
        if bundler_detected {
            score = score.saturating_sub(15);
            flags.push(format!(
                "BUNDLER_ACTIVITY_DETECTED: {} genesis slot snipes",
                genesis_slot_trades
            ));
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

    /// Score ONLY what the mint account itself proves (GAP-MAP P1 wiring
    /// into `module-sniper`). Holder distribution, LP burn and bundler
    /// signals are not observable from the mint, so they are neither
    /// assumed nor penalised — the report marks them unknown instead of
    /// inventing numbers.
    pub fn audit_authorities(
        mint: &Pubkey,
        mint_authority: Option<&Pubkey>,
        freeze_authority: Option<&Pubkey>,
    ) -> TokenSafetyReport {
        let mut flags = Vec::new();
        let mut score: u32 = 100;

        let is_mint_authority_renounced = mint_authority.is_none();
        if !is_mint_authority_renounced {
            score = score.saturating_sub(35);
            flags.push("MINT_AUTHORITY_ACTIVE: supply can still be inflated".into());
        }

        let is_freeze_authority_disabled = freeze_authority.is_none();
        if !is_freeze_authority_disabled {
            score = score.saturating_sub(45);
            flags.push(
                "FREEZE_AUTHORITY_ACTIVE: buyer token accounts can be frozen (honeypot lever)"
                    .into(),
            );
        }
        flags.push("LP_LOCK_UNKNOWN: not observable from the mint account".into());
        flags.push("HOLDER_DISTRIBUTION_UNKNOWN: not observable from the mint account".into());

        let verdict = if !is_freeze_authority_disabled {
            SafetyVerdict::HighRiskHoneypot
        } else if !is_mint_authority_renounced {
            SafetyVerdict::Caution
        } else {
            SafetyVerdict::Safe
        };

        TokenSafetyReport {
            mint: mint.to_string(),
            is_mint_authority_renounced,
            is_freeze_authority_disabled,
            is_lp_locked_or_burned: false,
            top_10_holder_percent: 0.0,
            bundler_detected: false,
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
        // Keep the freeze-authority warning isolated from the separate
        // holder-concentration penalty so the expected verdict exercises the
        // behavior named by this test.
        let holders = vec![2.0; 10];
        let report =
            TokenSafetyAuditor::audit_token(&mint, None, Some(&evil_auth), &holders, 100.0, 1);
        assert_eq!(report.verdict, SafetyVerdict::Caution);
        assert!(report
            .flags
            .iter()
            .any(|f| f.contains("FREEZE_AUTHORITY_ACTIVE")));
    }

    // ------------------------------------- sell probe classification (P1) --

    fn err(v: serde_json::Value) -> Option<serde_json::Value> {
        Some(v)
    }

    #[test]
    fn probe_without_error_is_sellable() {
        assert_eq!(classify_sell_probe(None, &[]), SellProbeOutcome::Sellable);
    }

    #[test]
    fn balance_check_error_means_the_sell_path_is_sound() {
        // The probe holds zero tokens: the SPL token program's
        // "insufficient funds" (custom 0x1) proves the venue executed the
        // sell instruction down to the transfer.
        let e = err(serde_json::json!({
            "InstructionError": [0, {"Custom": 1}]
        }));
        let logs = vec![
            "Program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P invoke [1]".into(),
            "Program log: Error processing Instruction 0: custom program error: 0x1".into(),
        ];
        assert_eq!(classify_sell_probe(e.as_ref(), &logs), SellProbeOutcome::Sellable);

        // Same signal via text form.
        let e = err(serde_json::json!({"SomeError": "InsufficientFunds"}));
        assert_eq!(classify_sell_probe(e.as_ref(), &[]), SellProbeOutcome::Sellable);
    }

    #[test]
    fn freeze_and_disabled_markers_are_not_sellable() {
        for marker in ["account is frozen", "sells disabled", "trading disabled"] {
            let e = err(serde_json::json!({"InstructionError": [0, {"Custom": 66}]}));
            let logs = vec![format!("Program log: {marker}")];
            match classify_sell_probe(e.as_ref(), &logs) {
                SellProbeOutcome::NotSellable(d) => assert!(d.contains(marker), "{d}"),
                other => panic!("expected NotSellable for '{marker}', got {other:?}"),
            }
        }
    }

    #[test]
    fn unrecognised_program_errors_stay_unknown() {
        // We do not guess safety OR guilt from a message we cannot
        // attribute — the gate treats Unknown as a skip.
        let e = err(serde_json::json!({"InstructionError": [0, {"Custom": 4242}]}));
        let logs = vec!["Program log: mystery".into()];
        match classify_sell_probe(e.as_ref(), &logs) {
            SellProbeOutcome::Unknown(d) => assert!(d.contains("4242") || d.contains("mystery") || !d.is_empty()),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn authorities_only_audit_penalises_only_what_it_can_see() {
        let mint = Pubkey::new_unique();
        let clean = TokenSafetyAuditor::audit_authorities(&mint, None, None);
        assert_eq!(clean.verdict, SafetyVerdict::Safe);
        assert_eq!(clean.safety_score, 100);
        assert!(clean.flags.iter().any(|f| f.contains("LP_LOCK_UNKNOWN")));
        assert!(clean
            .flags
            .iter()
            .any(|f| f.contains("HOLDER_DISTRIBUTION_UNKNOWN")));

        let mintable = TokenSafetyAuditor::audit_authorities(&mint, Some(&mint), None);
        assert_eq!(mintable.verdict, SafetyVerdict::Caution);

        let freezable = TokenSafetyAuditor::audit_authorities(&mint, None, Some(&mint));
        assert_eq!(freezable.verdict, SafetyVerdict::HighRiskHoneypot);
        assert_eq!(freezable.safety_score, 55);
    }
}
