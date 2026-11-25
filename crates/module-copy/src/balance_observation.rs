//! Best-effort post-confirmation diagnostics for copy trades. This does not
//! reconstruct an individual swap, correct the estimated booking, or post a
//! fee/revenue ledger entry. RPC metadata and commitment are trust assumptions.

use bot_core::{events::AppEvent, models::ExecutionMode, state::Shared};
use solana_kit::{execute::ExecStatus, rpc::Rpc};
use solana_sdk::{pubkey::Pubkey, signature::Signature};
use std::time::Duration;

/// Observe transaction-wide balance changes only for a confirmed live result.
/// The comparison to the modeled token quantity is diagnostic, not a proof of
/// the fill. A bounded timeout or incomplete metadata is explicitly unresolved.
#[allow(clippy::too_many_arguments)]
pub async fn audit_copy_balances(
    state: &Shared,
    rpc: &Rpc,
    mode: ExecutionMode,
    status: ExecStatus,
    signature: Option<&str>,
    wallet: &Pubkey,
    mint: &Pubkey,
    intent_id: &str,
    expected_token_delta: i128,
) {
    if mode != ExecutionMode::Live || status != ExecStatus::Confirmed {
        return;
    }
    let observation = async {
        let signature = signature
            .and_then(|s| s.parse::<Signature>().ok())
            .ok_or(())?;
        let transaction = rpc
            .get_transaction(&signature)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        solana_kit::confirmed_balances::confirmed_balance_deltas(
            &transaction,
            &signature,
            wallet,
            mint,
        )
        .map_err(|_| ())
    };
    let outcome = match tokio::time::timeout(Duration::from_secs(2), observation).await {
        Ok(Ok(delta)) => format!(
            "status=observed_transaction_deltas slot={} token_delta_raw={} token_decimals={} expected_token_delta_raw={} matches_expected={} inferred_zero_token_rows={} wallet_sol_delta_lamports={} network_fee_lamports={} wallet_is_fee_payer={} scope=whole_transaction not_fill_or_fee_proof=true",
            delta.slot, delta.token_delta_raw, delta.token_decimals, expected_token_delta,
            delta.token_delta_raw == expected_token_delta, delta.inferred_zero_token_rows,
            delta.wallet_sol_delta_lamports, delta.transaction_network_fee_lamports, delta.wallet_is_fee_payer
        ),
        Ok(Err(())) => "status=unresolved_transaction_deltas reason=unavailable_or_unverifiable not_fill_or_fee_proof=true".to_string(),
        Err(_) => "status=unresolved_transaction_deltas reason=timeout not_fill_or_fee_proof=true".to_string(),
    };
    state.events.publish(AppEvent::Audit {
        ts: chrono::Utc::now(),
        actor: "copy".into(),
        action: "copy.transaction_balance_observation".into(),
        target: Some(intent_id.to_owned()),
        outcome,
    });
}
