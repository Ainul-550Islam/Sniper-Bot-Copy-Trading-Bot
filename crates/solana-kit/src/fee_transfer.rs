//! Atomic on-chain platform-fee collection (GAP-MAP v2 P1).
//!
//! # What this is
//!
//! When the operator configures a fee vault (`[platform_fee]`), every
//! traded swap carries the platform's cut INSIDE the same transaction:
//!
//! * direct venue swaps (pump curve / PumpSwap / Raydium) get a system
//!   transfer `user → vault` appended to the swap instruction list —
//!   atomic by construction: the fee moves or the whole transaction fails;
//! * Jupiter-routed swaps use Jupiter's own [`PlatformFee`] mechanism on the
//!   quote, so the aggregator deducts the fee through its audited program
//!   instead of a side transfer.
//!
//! # What this is NOT
//!
//! It is not the accounting side: journal postings, tenant fee entries and
//! the revenue ledger live in `bot_core::billing::platform_fee` (micro-unit
//! HALF-UP math). This module only turns the same decision into something
//! the chain can execute.
//!
//! # Dust rule
//!
//! Fees below `min_fee_lamports` are SKIPPED entirely (no instruction, no
//! Jupiter fee): collecting dust costs more in transaction size and egress
//! than the dust is worth, and a fee of exactly zero must never emit an
//! empty transfer.

use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_system_interface::instruction as system_instruction;

use bot_core::config::PlatformFeeConfig;

use crate::jupiter::QuoteRequest;

/// A resolved fee decision for one trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformFeeDecision {
    /// The lamports to collect (0 = skip; never emit a zero transfer).
    pub lamports: u64,
    /// Why the fee is what it is (audit/metrics text).
    pub reason: FeeSkipReason,
}

/// Why a fee decision came out the way it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeSkipReason {
    /// Fee charged as computed.
    Charged,
    /// Collection is not configured (no vault or zero bps).
    Disabled,
    /// The computed fee is dust (below `min_fee_lamports`).
    Dust,
}

/// Compute the platform fee for a swap notional, in lamports.
///
/// HALF-UP rounding on `notional * bps / 10_000`, saturating on overflow:
/// a malformed notional can never panic or wrap into a tiny fee. The dust
/// gate applies LAST — a fee that rounds to something below the minimum is
/// skipped entirely, never collected partially.
pub fn compute_fee(notional_lamports: u64, cfg: &PlatformFeeConfig) -> PlatformFeeDecision {
    let vault_configured = !cfg.vault.trim().is_empty();
    if !vault_configured || cfg.bps == 0 {
        return PlatformFeeDecision {
            lamports: 0,
            reason: FeeSkipReason::Disabled,
        };
    }
    // (notional * bps) / 10_000, HALF-UP. Use u128 so a huge notional
    // (u64::MAX lamports) cannot overflow the multiplication.
    let product = (notional_lamports as u128) * (cfg.bps as u128);
    let quotient = product / 10_000;
    let remainder = product % 10_000;
    let mut fee = quotient;
    if remainder * 2 >= 10_000 {
        fee += 1; // HALF-UP
    }
    let fee = u64::try_from(fee).unwrap_or(u64::MAX);
    if fee == 0 || fee < cfg.min_fee_lamports {
        return PlatformFeeDecision {
            lamports: 0,
            reason: FeeSkipReason::Dust,
        };
    }
    PlatformFeeDecision {
        lamports: fee,
        reason: FeeSkipReason::Charged,
    }
}

/// Parse the configured vault. Invalid config is a skip, never a panic:
/// a mistyped vault must not break trading, it must disable collection
/// loudly (the metric/log happens where this is called).
pub fn vault_pubkey(cfg: &PlatformFeeConfig) -> Option<Pubkey> {
    let trimmed = cfg.vault.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<Pubkey>().ok()
}

/// Build the atomic fee transfer for a direct-venue swap.
///
/// Returns `None` whenever nothing should be appended (collection
/// disabled, dust fee, unparseable vault). The caller appends the
/// instruction to the SAME transaction as the swap — that is what makes
/// the fee atomic with the trade.
pub fn build_fee_transfer_ix(
    payer: &Pubkey,
    notional_lamports: u64,
    cfg: &PlatformFeeConfig,
) -> Option<(Instruction, PlatformFeeDecision)> {
    let decision = compute_fee(notional_lamports, cfg);
    if decision.lamports == 0 {
        return None;
    }
    let vault = vault_pubkey(cfg)?;
    let ix = system_instruction::transfer(payer, &vault, decision.lamports);
    Some((ix, decision))
}

/// Apply a Jupiter fee using an explicitly supplied SOL-denominated notional.
/// This avoids treating a token sell's raw input units as lamports. The
/// configured address must be an initialized SPL token account for the
/// fee mint; the caller must validate that account before live trading.
pub fn apply_jupiter_fee_for_sol_notional(
    mut request: QuoteRequest,
    cfg: &PlatformFeeConfig,
    notional_lamports: u64,
) -> bot_core::error::BotResult<(QuoteRequest, PlatformFeeDecision)> {
    let decision = compute_fee(notional_lamports, cfg);
    if decision.reason != FeeSkipReason::Charged {
        // A reused request may contain fee fields from a prior assessment.
        // Disabled and dust decisions must never retain an old charge.
        request.platform_fee_bps = None;
        request.platform_fee_account = None;
        return Ok((request, decision));
    }
    // The validated fee account is denominated in WSOL. Jupiter Metis
    // permits either pair mint on ExactIn but only the input on ExactOut.
    // Reject incompatible requests before asking the provider for a quote.
    let wsol = *crate::consts::WSOL_MINT;
    let allowed = match request.swap_mode {
        crate::jupiter::SwapMode::ExactIn => {
            request.input_mint == wsol || request.output_mint == wsol
        }
        crate::jupiter::SwapMode::ExactOut => request.input_mint == wsol,
    };
    if !allowed {
        return Err(bot_core::error::BotError::invalid(
            "Jupiter WSOL fee account is incompatible with the requested swap pair or mode",
        ));
    }
    let account = cfg
        .jupiter_fee_account
        .trim()
        .parse::<Pubkey>()
        .map_err(|_| {
            bot_core::error::BotError::invalid(
                "invalid or missing Jupiter platform fee token account",
            )
        })?;
    Ok((request.platform_fee(cfg.bps, account), decision))
}

/// Validate a Jupiter fee destination as an initialized, unfrozen WSOL
/// SPL-token account. This does not prove ownership or future settlement.
pub fn validate_wsol_fee_account(
    account: &solana_sdk::account::Account,
) -> bot_core::error::BotResult<()> {
    use bot_core::error::BotError;
    if account.owner != *crate::consts::TOKEN_PROGRAM {
        return Err(BotError::invalid(
            "Jupiter WSOL fee account must be owned by the SPL Token program",
        ));
    }
    let token = crate::tokens::TokenAccountInfo::parse(&account.data)?;
    if token.mint != *crate::consts::WSOL_MINT || token.state != 1 {
        return Err(BotError::invalid(
            "Jupiter fee account must be initialized, unfrozen and denominated in WSOL",
        ));
    }
    Ok(())
}

/// Prepare and validate a Jupiter request before an executable quote. The
/// notional must be SOL lamports, not token raw units. Errors are fail-closed.
pub async fn prepare_jupiter_fee_request(
    rpc: &crate::rpc::Rpc,
    request: QuoteRequest,
    cfg: &PlatformFeeConfig,
    notional_lamports: u64,
) -> bot_core::error::BotResult<(QuoteRequest, PlatformFeeDecision)> {
    use bot_core::error::BotError;
    let (request, decision) = apply_jupiter_fee_for_sol_notional(request, cfg, notional_lamports)?;
    if decision.reason == FeeSkipReason::Charged {
        let key = request
            .platform_fee_account
            .ok_or_else(|| BotError::invalid("missing Jupiter fee account"))?;
        let account = rpc
            .get_account(&key)
            .await?
            .ok_or_else(|| BotError::invalid("Jupiter fee token account does not exist"))?;
        validate_wsol_fee_account(&account)?;
    }
    Ok((request, decision))
}

/// Apply the platform fee to a Jupiter quote request (aggregator path).
///
/// Jupiter deducts the fee inside its own swap program when
/// `platform_fee_bps` + `platform_fee_account` are set on the quote — no
/// side transfer needed. Dust and disabled configurations return the
/// request unchanged.
pub fn apply_jupiter_fee(mut request: QuoteRequest, cfg: &PlatformFeeConfig) -> QuoteRequest {
    let decision = compute_fee(request.amount, cfg);
    if decision.lamports == 0 {
        return request;
    }
    let Ok(vault) = cfg.jupiter_fee_account.trim().parse::<Pubkey>() else {
        return request;
    };
    // Jupiter takes the fee in bps of the INPUT amount; pass the same bps
    // the decision was made with so the aggregator and the accounting side
    // agree on the rate.
    request = request.platform_fee(cfg.bps, vault);
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn cfg(vault: &str, bps: u64, min: u64) -> PlatformFeeConfig {
        PlatformFeeConfig {
            vault: vault.to_string(),
            jupiter_fee_account: vault.to_string(),
            bps,
            min_fee_lamports: min,
        }
    }

    const VAULT: &str = "11111111111111111111111111111111";

    #[test]
    fn jupiter_sell_fee_uses_sol_notional_not_token_raw_amount() {
        let policy = cfg(VAULT, 100, 100);
        let token = Pubkey::new_unique();
        let request = QuoteRequest::new(token, *crate::consts::WSOL_MINT, 1);
        let (request, decision) =
            apply_jupiter_fee_for_sol_notional(request, &policy, 1_000_000).unwrap();
        assert_eq!(decision.lamports, 10_000);
        assert_eq!(request.platform_fee_bps, Some(100));
        assert_eq!(request.amount, 1);
        let invalid = cfg("not a pubkey", 100, 1);
        assert!(apply_jupiter_fee_for_sol_notional(
            QuoteRequest::new(token, *crate::consts::WSOL_MINT, 1),
            &invalid,
            1_000_000
        )
        .is_err());
    }

    #[test]
    fn fee_is_half_up_bps_of_the_notional() {
        // 1 SOL at 100 bps (1%) = exactly 0.01 SOL.
        let d = compute_fee(1_000_000_000, &cfg(VAULT, 100, 1));
        assert_eq!(d.lamports, 10_000_000);
        assert_eq!(d.reason, FeeSkipReason::Charged);

        // HALF-UP: 150 lamports at 100 bps = 1.5 → rounds to 2.
        let d = compute_fee(150, &cfg(VAULT, 100, 1));
        assert_eq!(d.lamports, 2);
        // 149 lamports at 100 bps = 1.49 → rounds to 1.
        let d = compute_fee(149, &cfg(VAULT, 100, 1));
        assert_eq!(d.lamports, 1);
        // Exact half rounds UP: 50 lamports at 100 bps = 0.5 → 1.
        let d = compute_fee(50, &cfg(VAULT, 100, 1));
        assert_eq!(d.lamports, 1);
    }

    #[test]
    fn zero_rounded_fee_is_dust_even_with_zero_minimum() {
        let policy = cfg(VAULT, 1, 0);
        for notional in [0, 1, 100] {
            let decision = compute_fee(notional, &policy);
            assert_eq!(decision.lamports, 0);
            assert_eq!(decision.reason, FeeSkipReason::Dust);
            let req = QuoteRequest::new(*crate::consts::WSOL_MINT, Pubkey::new_unique(), notional);
            let (req, decision) =
                apply_jupiter_fee_for_sol_notional(req, &policy, notional).unwrap();
            assert_eq!(decision.reason, FeeSkipReason::Dust);
            assert!(req.platform_fee_bps.is_none());
        }
    }

    #[test]
    fn charged_jupiter_fee_requires_wsol_in_the_allowed_swap_leg() {
        use crate::jupiter::SwapMode;
        let policy = cfg(VAULT, 100, 1);
        let token_a = Pubkey::new_unique();
        let token_b = Pubkey::new_unique();
        let wsol = *crate::consts::WSOL_MINT;
        let plain = QuoteRequest::new(token_a, token_b, 1_000_000);
        assert!(apply_jupiter_fee_for_sol_notional(plain, &policy, 1_000_000).is_err());
        for (input, output) in [(wsol, token_a), (token_a, wsol)] {
            let request = QuoteRequest::new(input, output, 1_000_000);
            assert!(apply_jupiter_fee_for_sol_notional(request, &policy, 1_000_000).is_ok());
        }
        let mut out_input_wsol = QuoteRequest::new(wsol, token_a, 1_000_000);
        out_input_wsol.swap_mode = SwapMode::ExactOut;
        assert!(apply_jupiter_fee_for_sol_notional(out_input_wsol, &policy, 1_000_000).is_ok());
        let mut out_output_wsol = QuoteRequest::new(token_a, wsol, 1_000_000);
        out_output_wsol.swap_mode = SwapMode::ExactOut;
        assert!(apply_jupiter_fee_for_sol_notional(out_output_wsol, &policy, 1_000_000).is_err());
    }

    #[test]
    fn skipped_fee_clears_preexisting_jupiter_fields() {
        let old_account = Pubkey::new_unique();
        for policy in [cfg("", 0, 0), cfg(VAULT, 1, 1000)] {
            let request = QuoteRequest::new(*crate::consts::WSOL_MINT, Pubkey::new_unique(), 100)
                .platform_fee(200, old_account);
            let (request, decision) =
                apply_jupiter_fee_for_sol_notional(request, &policy, 100).unwrap();
            assert!(matches!(
                decision.reason,
                FeeSkipReason::Disabled | FeeSkipReason::Dust
            ));
            assert_eq!(request.platform_fee_bps, None);
            assert_eq!(request.platform_fee_account, None);
        }
    }

    #[test]
    fn disabled_collection_yields_zero_without_a_vault_or_bps() {
        assert_eq!(
            compute_fee(1_000_000_000, &cfg("", 100, 1)).reason,
            FeeSkipReason::Disabled
        );
        assert_eq!(
            compute_fee(1_000_000_000, &cfg("   ", 100, 1)).reason,
            FeeSkipReason::Disabled
        );
        assert_eq!(
            compute_fee(1_000_000_000, &cfg(VAULT, 0, 1)).reason,
            FeeSkipReason::Disabled
        );
        assert_eq!(compute_fee(1_000_000_000, &cfg("", 100, 1)).lamports, 0);
    }

    #[test]
    fn dust_fees_are_skipped_entirely() {
        // 1000 lamports at 1 bp = 0.1 lamport → rounds to 0 → dust.
        let d = compute_fee(1_000, &cfg(VAULT, 1, 1));
        assert_eq!(d.lamports, 0);
        assert_eq!(d.reason, FeeSkipReason::Dust);
        // Below the configured minimum even a "real" fee is skipped.
        let d = compute_fee(100_000, &cfg(VAULT, 100, 10_000));
        assert_eq!(d.lamports, 0);
        assert_eq!(d.reason, FeeSkipReason::Dust);
        // At the minimum it is charged.
        let d = compute_fee(1_000_000, &cfg(VAULT, 100, 10_000));
        assert_eq!(d.lamports, 10_000);
        assert_eq!(d.reason, FeeSkipReason::Charged);
    }

    #[test]
    fn fee_never_overflows_on_extreme_notional() {
        let d = compute_fee(u64::MAX, &cfg(VAULT, 10_000, 1));
        // 100% of u64::MAX is u64::MAX — saturates, never panics.
        assert_eq!(d.lamports, u64::MAX);
        assert_eq!(d.reason, FeeSkipReason::Charged);
        // Huge bps are clamped by config parsing, but compute_fee itself
        // must stay total even if handed one.
        let d = compute_fee(u64::MAX, &cfg(VAULT, u64::MAX, 1));
        assert_eq!(d.reason, FeeSkipReason::Charged);
    }

    #[test]
    fn zero_notional_is_dust() {
        let d = compute_fee(0, &cfg(VAULT, 100, 1));
        assert_eq!(d.lamports, 0);
        assert_eq!(d.reason, FeeSkipReason::Dust);
    }

    #[test]
    fn build_ix_is_none_when_there_is_nothing_to_collect() {
        let payer = Pubkey::new_unique();
        assert!(build_fee_transfer_ix(&payer, 1_000_000_000, &cfg("", 100, 1)).is_none());
        assert!(build_fee_transfer_ix(&payer, 10, &cfg(VAULT, 100, 10_000)).is_none());
        // Unparseable vault: skip, never panic.
        assert!(
            build_fee_transfer_ix(&payer, 1_000_000_000, &cfg("not-a-pubkey", 100, 1)).is_none()
        );
    }

    #[test]
    fn build_ix_transfers_the_exact_fee_to_the_vault() {
        let payer = Pubkey::new_unique();
        let vault = Pubkey::from_str(VAULT).unwrap();
        let (ix, decision) =
            build_fee_transfer_ix(&payer, 2_000_000_000, &cfg(VAULT, 50, 1)).unwrap();
        assert_eq!(decision.lamports, 10_000_000); // 0.5% of 2 SOL
        assert_eq!(decision.reason, FeeSkipReason::Charged);
        // A system transfer carries the payer, the vault and the amount.
        assert_eq!(ix.program_id, solana_system_interface::program::id());
        assert_eq!(ix.accounts.len(), 2);
        assert_eq!(ix.accounts[0].pubkey, payer);
        assert_eq!(ix.accounts[1].pubkey, vault);
        let lamports = u64::from_le_bytes(ix.data[4..12].try_into().unwrap());
        assert_eq!(lamports, 10_000_000);
    }

    #[test]
    fn jupiter_fee_is_applied_only_when_chargeable() {
        let wsol = Pubkey::new_unique();
        let token = Pubkey::new_unique();
        let chargeable = QuoteRequest::new(wsol, token, 1_000_000_000);
        let with_fee = apply_jupiter_fee(chargeable.clone(), &cfg(VAULT, 100, 1));
        assert_eq!(with_fee.platform_fee_bps, Some(100));
        assert_eq!(
            with_fee.platform_fee_account,
            Some(Pubkey::from_str(VAULT).unwrap())
        );

        // Dust: request untouched.
        let tiny = QuoteRequest::new(wsol, token, 10);
        let untouched = apply_jupiter_fee(tiny.clone(), &cfg(VAULT, 100, 10_000));
        assert_eq!(untouched.platform_fee_bps, None);
        assert_eq!(untouched.platform_fee_account, None);

        // Disabled: request untouched.
        let untouched = apply_jupiter_fee(tiny, &cfg("", 100, 1));
        assert_eq!(untouched.platform_fee_bps, None);
    }
}
