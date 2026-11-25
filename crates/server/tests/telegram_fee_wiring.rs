//! Offline fee-request arithmetic tests. These do not claim live Jupiter,
//! ledger posting or the Telegram command pipeline was exercised.
use bot_core::config::PlatformFeeConfig;
use solana_kit::consts::WSOL_MINT;
use solana_kit::fee_transfer::{apply_jupiter_fee_for_sol_notional, FeeSkipReason};
use solana_kit::jupiter::QuoteRequest;
use solana_sdk::pubkey::Pubkey;

#[test]
fn buy_and_sell_fee_requests_use_sol_notional() {
    let account = Pubkey::new_unique();
    let token = Pubkey::new_unique();
    let config = PlatformFeeConfig {
        vault: account.to_string(),
        jupiter_fee_account: account.to_string(),
        bps: 100,
        min_fee_lamports: 1,
    };
    for req in [
        QuoteRequest::new(*WSOL_MINT, token, 1_000_000),
        QuoteRequest::new(token, *WSOL_MINT, 17),
    ] {
        let (request, decision) =
            apply_jupiter_fee_for_sol_notional(req, &config, 1_000_000).unwrap();
        assert_eq!(decision.lamports, 10_000);
        assert_eq!(request.platform_fee_bps, Some(100));
        assert_eq!(request.platform_fee_account, Some(account));
    }
}

#[test]
fn disabled_or_dust_fee_does_not_attach_a_fee_account() {
    let token = Pubkey::new_unique();
    for config in [
        PlatformFeeConfig {
            vault: String::new(),
            jupiter_fee_account: String::new(),
            bps: 0,
            min_fee_lamports: 1,
        },
        PlatformFeeConfig {
            vault: Pubkey::new_unique().to_string(),
            jupiter_fee_account: Pubkey::new_unique().to_string(),
            bps: 1,
            min_fee_lamports: 1000,
        },
    ] {
        let (request, decision) = apply_jupiter_fee_for_sol_notional(
            QuoteRequest::new(token, *WSOL_MINT, 100),
            &config,
            100,
        )
        .unwrap();
        assert!(matches!(
            decision.reason,
            FeeSkipReason::Disabled | FeeSkipReason::Dust
        ));
        assert_eq!(request.platform_fee_bps, None);
        assert_eq!(request.platform_fee_account, None);
    }
}

#[test]
fn disabled_and_dust_policy_remove_preexisting_quote_fee_fields() {
    let input = *WSOL_MINT;
    let output = Pubkey::new_unique();
    let stale_account = Pubkey::new_unique();
    for cfg in [
        PlatformFeeConfig {
            vault: String::new(),
            jupiter_fee_account: String::new(),
            bps: 0,
            min_fee_lamports: 0,
        },
        PlatformFeeConfig {
            vault: Pubkey::new_unique().to_string(),
            jupiter_fee_account: Pubkey::new_unique().to_string(),
            bps: 1,
            min_fee_lamports: 1000,
        },
    ] {
        let prior = QuoteRequest::new(input, output, 100).platform_fee(200, stale_account);
        let (prepared, decision) = apply_jupiter_fee_for_sol_notional(prior, &cfg, 100).unwrap();
        assert!(matches!(
            decision.reason,
            FeeSkipReason::Disabled | FeeSkipReason::Dust
        ));
        assert_eq!(prepared.platform_fee_bps, None);
        assert_eq!(prepared.platform_fee_account, None);
    }
}
