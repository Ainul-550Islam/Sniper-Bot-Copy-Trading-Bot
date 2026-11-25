//! Atomic SOL fee instruction preparation for copy routes.
//!
//! Fee instruction preparation alone is not proof that a transaction landed.
//! A separately verified direct fee leg is posted as a tenant fee expense in
//! the global accounting ledger; this is not platform revenue or a payout.
use bot_core::accounting::{AccountingEvent, Applied, EventKind};
use bot_core::config::PlatformFeeConfig;
use bot_core::error::{BotError, BotResult};
use bot_core::events::AppEvent;
use bot_core::models::{BotModule, ExecutionMode, Venue};
use bot_core::state::Shared;
use chrono::Utc;
use solana_kit::confirmed_fee::ConfirmedDirectFee;
use solana_kit::fee_transfer::{build_fee_transfer_ix, compute_fee, FeeSkipReason};
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, signature::Signature};

/// Prepare a direct-route fee and audit the assessment. Invalid configured
/// vaults fail closed rather than silently omitting an otherwise charged fee.
pub fn direct_fee_instruction(
    state: &Shared,
    payer: &Pubkey,
    notional_lamports: u64,
    cfg: &PlatformFeeConfig,
    intent_id: &str,
) -> BotResult<Option<Instruction>> {
    let decision = compute_fee(notional_lamports, cfg);
    // A System Program transfer from an account to itself cannot establish
    // platform revenue. Refuse before broadcasting rather than later treating
    // the resulting no-op as a collected fee.
    if decision.reason == FeeSkipReason::Charged
        && solana_kit::fee_transfer::vault_pubkey(cfg).as_ref() == Some(payer)
    {
        state.events.publish(AppEvent::Audit {
            ts: chrono::Utc::now(),
            actor: "copy".into(),
            action: "copy.platform_fee.assessed".into(),
            target: Some(intent_id.to_string()),
            outcome: format!(
                "reason={:?} notional_lamports={} fee_lamports={} status=rejected_self_transfer",
                decision.reason, notional_lamports, decision.lamports
            ),
        });
        return Err(BotError::invalid(
            "copy platform fee vault must differ from payer",
        ));
    }
    let instruction = build_fee_transfer_ix(payer, notional_lamports, cfg);
    if decision.reason == FeeSkipReason::Charged && instruction.is_none() {
        state.events.publish(AppEvent::Audit {
            ts: chrono::Utc::now(),
            actor: "copy".into(),
            action: "copy.platform_fee.assessed".into(),
            target: Some(intent_id.to_string()),
            outcome: format!(
                "reason={:?} notional_lamports={} fee_lamports={} status=rejected_invalid_vault",
                decision.reason, notional_lamports, decision.lamports
            ),
        });
        return Err(BotError::invalid(
            "copy platform fee vault is not a valid Solana public key",
        ));
    }
    state.events.publish(AppEvent::Audit {
        ts: chrono::Utc::now(),
        actor: "copy".into(),
        action: "copy.platform_fee.assessed".into(),
        target: Some(intent_id.to_string()),
        outcome: format!(
            "reason={:?} notional_lamports={} fee_lamports={} status=prepared",
            decision.reason, notional_lamports, decision.lamports
        ),
    });
    Ok(instruction.map(|(instruction, _)| instruction))
}

/// Record one locally verified native-SOL fee leg as a tenant fee expense in
/// the global ledger. The idempotency identity is the exact transaction
/// signature, destination vault and lamport amount, so a confirmed replay
/// cannot charge the wallet's accounting book twice. This is NOT platform
/// revenue, a vault reconciliation, or proof of a later payout.
pub(crate) async fn book_verified_direct_fee(
    state: &Shared,
    proof: &ConfirmedDirectFee,
    intent_id: &str,
) -> Applied {
    // One transaction signature is one fact. Payer, vault and amount remain
    // in the payload, so a replay claiming different economics under the
    // same signature is a conflict, not a second fee event.
    let reference_id = format!("confirmed-direct-fee:{}", proof.signature);
    let event = AccountingEvent {
        kind: EventKind::Fee,
        module: BotModule::Copy,
        venue: Venue::PumpFun,
        wallet: proof.payer.to_string(),
        strategy: "copy".into(),
        asset: "SOL".into(),
        quote_asset: "SOL".into(),
        side: None,
        quantity: 0.0,
        price: None,
        quote_amount: 0.0,
        fee: bot_core::maths::lamports_to_sol(proof.lamports),
        mode: ExecutionMode::Live,
        reference_id,
        correlation_id: Some(format!("confirmed-direct-fee:{}", proof.signature)),
        position_id: None,
        trade_id: None,
        counterparty_wallet: Some(proof.vault.to_string()),
        ts: Utc::now(),
        detail: format!(
            "intent={} slot={} payer={} vault={} verified_direct_fee_lamports={}",
            intent_id.trim(),
            proof.slot,
            proof.payer,
            proof.vault,
            proof.lamports
        ),
    };
    state.submit_ledger_event(BotModule::Copy, event).await
}

fn applied_label(applied: &Applied) -> &'static str {
    match applied {
        Applied::New(_) => "new",
        Applied::Duplicate => "duplicate",
        Applied::Rejected(_) => "rejected",
    }
}

/// Verify a direct fee leg only after the execution reports a live
/// confirmation, then post that exact leg to the global ledger. A missing,
/// ambiguous, unverifiable or tampered transaction remains unresolved and
/// books no fee. RPC data, commitment and provider honesty remain trusted.
pub async fn audit_confirmed_direct_fee(
    state: &Shared,
    rpc: &solana_kit::rpc::Rpc,
    payer: &Pubkey,
    notional_lamports: u64,
    cfg: &PlatformFeeConfig,
    intent_id: &str,
    result: &solana_kit::execute::ExecutionResult,
) {
    // Classify the finished attempt, not the current mutable mode: a kill
    // switch may have changed live to simulate after broadcast.
    if result.paper || result.status != solana_kit::execute::ExecStatus::Confirmed {
        return;
    }
    let decision = compute_fee(notional_lamports, cfg);
    if decision.reason != FeeSkipReason::Charged {
        return;
    }
    let proof = async {
        // A signature that never left the process cannot establish a fee,
        // even if a test fixture or stale result carries one.
        let broadcast = result
            .broadcast_signature()
            .ok_or_else(|| BotError::invalid("missing confirmed broadcast signature"))?;
        let signature: Signature = broadcast
            .parse()
            .map_err(|_| BotError::invalid("invalid confirmed signature"))?;
        let vault = solana_kit::fee_transfer::vault_pubkey(cfg)
            .ok_or_else(|| BotError::invalid("invalid fee vault"))?;
        let transaction = rpc
            .get_transaction(&signature)
            .await?
            .ok_or_else(|| BotError::invalid("confirmed transaction unavailable"))?;
        solana_kit::confirmed_fee::verify_confirmed_direct_fee(
            &transaction,
            &signature,
            payer,
            &vault,
            decision.lamports,
        )
    }
    .await;
    let outcome = match proof {
        Ok(verified) => {
            let applied = book_verified_direct_fee(state, &verified, intent_id).await;
            format!(
                "status=verified_direct_leg slot={} fee_lamports={} ledger={}",
                verified.slot,
                verified.lamports,
                applied_label(&applied)
            )
        }
        Err(error) => format!(
            "status=unresolved_direct_leg fee_lamports={} error={error}",
            decision.lamports
        ),
    };
    state.events.publish(AppEvent::Audit {
        ts: chrono::Utc::now(),
        actor: "copy".into(),
        action: "copy.platform_fee.confirmation".into(),
        target: Some(intent_id.to_string()),
        outcome,
    });
}

/// Apply Jupiter fee using a SOL notional, checking the configured destination
/// is an initialized WSOL token account before producing a fee-bearing quote.
/// Audit means prepared, not collected; reconciliation must establish landing.
pub async fn jupiter_fee_request(
    state: &Shared,
    rpc: &solana_kit::rpc::Rpc,
    request: solana_kit::jupiter::QuoteRequest,
    notional_lamports: u64,
    cfg: &PlatformFeeConfig,
    intent_id: &str,
    actor: &str,
) -> BotResult<solana_kit::jupiter::QuoteRequest> {
    let prepared =
        solana_kit::fee_transfer::prepare_jupiter_fee_request(rpc, request, cfg, notional_lamports)
            .await;
    let (request, decision) = match prepared {
        Ok(value) => value,
        Err(error) => {
            let decision = compute_fee(notional_lamports, cfg);
            state.events.publish(AppEvent::Audit {
                ts: chrono::Utc::now(),
                actor: actor.into(),
                action: format!("{actor}.platform_fee.assessed"),
                target: Some(intent_id.to_string()),
                outcome: format!(
                    "reason={:?} notional_lamports={} fee_lamports={} status=rejected_jupiter_fee_preparation",
                    decision.reason, notional_lamports, decision.lamports
                ),
            });
            return Err(error);
        }
    };
    state.events.publish(AppEvent::Audit {
        ts: chrono::Utc::now(),
        actor: actor.into(),
        action: format!("{actor}.platform_fee.assessed"),
        target: Some(intent_id.to_string()),
        outcome: format!(
            "reason={:?} notional_lamports={} fee_lamports={} status=prepared",
            decision.reason, notional_lamports, decision.lamports
        ),
    });
    Ok(request)
}

/// Validate the account's token program, WSOL mint and initialized state.
pub fn validate_wsol_fee_account(account: &solana_sdk::account::Account) -> BotResult<()> {
    solana_kit::fee_transfer::validate_wsol_fee_account(account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::accounting::{Account, EventKind};
    use bot_core::config::AppConfig;
    use bot_core::state::AppState;
    use solana_sdk::signature::{Keypair, Signer};

    #[tokio::test]
    async fn verified_fee_is_exact_balanced_and_idempotent_per_signature() {
        let state = AppState::new(AppConfig::from_defaults());
        let payer = Keypair::new();
        let vault = Pubkey::new_unique();
        let proof = ConfirmedDirectFee {
            signature: payer.sign_message(b"verified platform fee"),
            slot: 77,
            payer: payer.pubkey(),
            vault,
            lamports: 12_345,
        };
        assert!(book_verified_direct_fee(&state, &proof, "intent-a")
            .await
            .is_new());
        assert_eq!(
            book_verified_direct_fee(&state, &proof, "intent-b").await,
            Applied::Duplicate
        );
        let mut conflicting = proof.clone();
        conflicting.lamports = 22_000;
        assert!(matches!(
            book_verified_direct_fee(&state, &conflicting, "intent-c").await,
            Applied::Rejected(_)
        ));

        let events = state.ledger().events().await;
        assert_eq!(events.len(), 1, "one transaction is one fee fact");
        let event = &events[0].event;
        assert_eq!(event.kind, EventKind::Fee);
        assert_eq!(event.module, BotModule::Copy);
        assert_eq!(event.venue, Venue::PumpFun);
        assert_eq!(event.mode, ExecutionMode::Live);
        assert_eq!(event.fee, bot_core::maths::lamports_to_sol(12_345));
        assert_eq!(event.quote_amount, 0.0);
        assert_eq!(
            event.counterparty_wallet.as_deref(),
            Some(vault.to_string().as_str())
        );
        let entry = state
            .ledger()
            .entry(&events[0].event_id)
            .await
            .expect("balanced fee entry");
        assert!(entry.is_balanced());
        assert!((entry.net(Account::Cash) + event.fee).abs() < 1e-12);
        assert!((entry.net(Account::Fees) - event.fee).abs() < 1e-12);
    }
}
