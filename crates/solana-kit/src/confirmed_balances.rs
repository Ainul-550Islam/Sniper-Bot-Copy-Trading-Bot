//! Conservative raw balance observations for a signed, successful Solana
//! transaction. These are transaction-wide deltas, not a swap-fill decoder:
//! rent, tips, transfers, other instructions and token extensions can affect
//! them. RPC metadata and commitment are trusted, not cryptographically
//! proven by the transaction signature. No ledger or revenue is posted here.

use std::collections::HashMap;
use std::str::FromStr;

use bot_core::error::{BotError, BotResult};
use solana_sdk::{message::VersionedMessage, pubkey::Pubkey, signature::Signature};
use solana_transaction_status::{
    option_serializer::OptionSerializer, EncodedConfirmedTransactionWithStatusMeta,
    UiTransactionTokenBalance,
};

/// Transaction-wide observed deltas, exact in raw units. A negative value
/// means the wallet lost units. These must not be interpreted as trade value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedBalanceDeltas {
    pub signature: Signature,
    pub slot: u64,
    pub wallet: Pubkey,
    pub mint: Pubkey,
    pub token_decimals: u8,
    pub token_delta_raw: i128,
    /// Rows whose zero pre/post token amount was inferred from SOL balances.
    pub inferred_zero_token_rows: usize,
    pub wallet_sol_delta_lamports: i128,
    /// The transaction network fee, not necessarily paid by `wallet`.
    pub transaction_network_fee_lamports: u64,
    pub wallet_is_fee_payer: bool,
}

/// Verify transaction identity and extract token-account balances owned by
/// `wallet` for `mint`. A missing pre/post token row is treated as zero only
/// when the corresponding SOL account balance is zero on that side and
/// positive on the other, consistent with creation/closure. This assumes
/// complete and truthful RPC metadata; zero lamports alone does not prove
/// the account's history. Missing balance lists or owners, ambiguous missing
/// rows, changing ownership/mint, inconsistent decimals, duplicate indices,
/// lookup tables and malformed metadata are refused. Even a successful
/// observation is not sufficient to post a fill, fee, or cash receipt.
pub fn confirmed_balance_deltas(
    response: &EncodedConfirmedTransactionWithStatusMeta,
    signature: &Signature,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> BotResult<ConfirmedBalanceDeltas> {
    let invalid = |message: &str| BotError::invalid(message);
    let meta = response
        .transaction
        .meta
        .as_ref()
        .ok_or_else(|| invalid("transaction metadata missing"))?;
    if meta.err.is_some() || meta.status.is_err() {
        return Err(invalid("transaction failed"));
    }
    let tx = response
        .transaction
        .transaction
        .decode()
        .ok_or_else(|| invalid("transaction bytes unavailable or invalid"))?;
    if tx.signatures.first() != Some(signature) || tx.verify_and_hash_message().is_err() {
        return Err(invalid(
            "transaction signature does not match or is invalid",
        ));
    }
    if let VersionedMessage::V0(message) = &tx.message {
        if !message.address_table_lookups.is_empty() {
            return Err(invalid("lookup-table transaction not supported"));
        }
    }
    let keys = tx.message.static_account_keys();
    let wallet_index = keys
        .iter()
        .position(|key| key == wallet)
        .ok_or_else(|| invalid("wallet absent from transaction"))?;
    if wallet_index >= usize::from(tx.message.header().num_required_signatures) {
        return Err(invalid("wallet is not a transaction signer"));
    }
    if meta.pre_balances.len() != keys.len() || meta.post_balances.len() != keys.len() {
        return Err(invalid("incomplete SOL balances"));
    }
    let pre = index_balances(&meta.pre_token_balances, keys.len())?;
    let post = index_balances(&meta.post_token_balances, keys.len())?;
    let mint_text = mint.to_string();
    let mut decimals = None;
    let mut delta = 0i128;
    let mut matched = 0usize;
    let mut inferred_zero_token_rows = 0usize;
    let indices: std::collections::BTreeSet<u8> = pre.keys().chain(post.keys()).copied().collect();
    for index in indices {
        let before = pre.get(&index);
        let after = post.get(&index);
        if !before.is_some_and(|row| row.mint == mint_text)
            && !after.is_some_and(|row| row.mint == mint_text)
        {
            continue;
        }
        let owner_of = |row: &UiTransactionTokenBalance| -> BotResult<Pubkey> {
            match &row.owner {
                OptionSerializer::Some(owner) => {
                    Pubkey::from_str(owner).map_err(|_| invalid("invalid token owner"))
                }
                _ => Err(invalid("token owner missing")),
            }
        };
        let before_owner = before.map(|r| owner_of(r)).transpose()?;
        let after_owner = after.map(|r| owner_of(r)).transpose()?;
        if before_owner != Some(*wallet) && after_owner != Some(*wallet) {
            continue;
        }
        let check_row = |row: &UiTransactionTokenBalance, owner: Option<Pubkey>| -> BotResult<()> {
            if owner != Some(*wallet) || row.mint != mint_text {
                return Err(invalid("token account identity changed"));
            }
            Ok(())
        };
        if let Some(row) = before {
            check_row(row, before_owner)?;
        }
        if let Some(row) = after {
            check_row(row, after_owner)?;
        }
        let row = before
            .or(after)
            .expect("index comes from one of the balance lists");
        let d = row.ui_token_amount.decimals;
        if before.is_some_and(|r| r.ui_token_amount.decimals != d)
            || after.is_some_and(|r| r.ui_token_amount.decimals != d)
            || decimals.is_some_and(|old| old != d)
        {
            return Err(invalid("inconsistent token decimals"));
        }
        decimals = Some(d);
        let account_index = usize::from(index);
        let amount = |row: Option<&&UiTransactionTokenBalance>,
                      missing_sol: u64,
                      present_sol: u64|
         -> BotResult<u64> {
            match row {
                Some(row) => row
                    .ui_token_amount
                    .amount
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid raw token amount")),
                None if missing_sol == 0 && present_sol > 0 => Ok(0),
                None => Err(invalid(
                    "wallet token account missing ambiguous pre or post balance",
                )),
            }
        };
        let pre_amount = amount(
            before,
            meta.pre_balances[account_index],
            meta.post_balances[account_index],
        )?;
        let post_amount = amount(
            after,
            meta.post_balances[account_index],
            meta.pre_balances[account_index],
        )?;
        inferred_zero_token_rows += usize::from(before.is_none()) + usize::from(after.is_none());
        delta = delta
            .checked_add(i128::from(post_amount) - i128::from(pre_amount))
            .ok_or_else(|| invalid("token delta overflow"))?;
        matched += 1;
    }
    if matched == 0 {
        return Err(invalid("no paired wallet token balances for mint"));
    }
    Ok(ConfirmedBalanceDeltas {
        signature: *signature,
        slot: response.slot,
        wallet: *wallet,
        mint: *mint,
        token_decimals: decimals.expect("matched account has decimals"),
        token_delta_raw: delta,
        inferred_zero_token_rows,
        wallet_sol_delta_lamports: i128::from(meta.post_balances[wallet_index])
            - i128::from(meta.pre_balances[wallet_index]),
        transaction_network_fee_lamports: meta.fee,
        wallet_is_fee_payer: wallet_index == 0,
    })
}

fn index_balances(
    balances: &OptionSerializer<Vec<UiTransactionTokenBalance>>,
    key_count: usize,
) -> BotResult<HashMap<u8, &UiTransactionTokenBalance>> {
    let rows = match balances {
        OptionSerializer::Some(rows) => rows,
        _ => return Err(BotError::invalid("token balance list missing")),
    };
    let mut indexed = HashMap::with_capacity(rows.len());
    for row in rows {
        if usize::from(row.account_index) >= key_count
            || indexed.insert(row.account_index, row).is_some()
        {
            return Err(BotError::invalid(
                "invalid or duplicate token account index",
            ));
        }
    }
    Ok(indexed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::{json, Value};
    use solana_sdk::{
        hash::Hash,
        instruction::{AccountMeta, Instruction},
        signature::{Keypair, Signer},
        transaction::Transaction,
    };

    fn row(index: usize, mint: Pubkey, owner: Pubkey, amount: u64, decimals: u8) -> Value {
        json!({"accountIndex": index, "mint": mint.to_string(), "owner": owner.to_string(),
            "uiTokenAmount": {"amount": amount.to_string(), "decimals": decimals, "uiAmount": null, "uiAmountString": ""}})
    }

    fn fixture() -> (
        EncodedConfirmedTransactionWithStatusMeta,
        Signature,
        Pubkey,
        Pubkey,
    ) {
        let payer = Keypair::new();
        let wallet = payer.pubkey();
        let mint = Pubkey::new_unique();
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let instruction = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(a, false), AccountMeta::new(b, false)],
            data: vec![],
        };
        let tx = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&wallet),
            &[&payer],
            Hash::new_unique(),
        );
        let keys = &tx.message.account_keys;
        let index_a = keys.iter().position(|k| *k == a).unwrap();
        let index_b = keys.iter().position(|k| *k == b).unwrap();
        let signature = tx.signatures[0];
        let mut pre_sol = vec![0u64; keys.len()];
        let mut post_sol = pre_sol.clone();
        pre_sol[0] = 1_000_000;
        post_sol[0] = 700_000;
        pre_sol[index_a] = 2_000_000;
        post_sol[index_a] = 2_000_000;
        pre_sol[index_b] = 2_000_000;
        post_sol[index_b] = 2_000_000;
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap());
        let response = serde_json::from_value(json!({"slot": 123, "transaction": [encoded, "base64"], "blockTime": null,
            "meta": {"err": null, "status": {"Ok": null}, "fee": 5000, "preBalances": pre_sol, "postBalances": post_sol,
                "preTokenBalances": [row(index_a,mint,wallet,100,6), row(index_b,mint,wallet,50,6)],
                "postTokenBalances": [row(index_a,mint,wallet,130,6), row(index_b,mint,wallet,40,6)]}})).unwrap();
        (response, signature, wallet, mint)
    }

    #[test]
    fn exact_transaction_wide_deltas_sum_multiple_accounts() {
        let (tx, sig, wallet, mint) = fixture();
        let result = confirmed_balance_deltas(&tx, &sig, &wallet, &mint).unwrap();
        assert_eq!(result.token_delta_raw, 20);
        assert_eq!(result.inferred_zero_token_rows, 0);
        assert_eq!(result.token_decimals, 6);
        assert_eq!(result.wallet_sol_delta_lamports, -300_000);
        assert_eq!(result.transaction_network_fee_lamports, 5000);
        assert!(result.wallet_is_fee_payer);
        assert_eq!(result.slot, 123);
    }

    #[test]
    fn refuses_identity_failures_and_unsuccessful_transaction() {
        let (mut tx, sig, wallet, mint) = fixture();
        assert!(confirmed_balance_deltas(&tx, &Signature::default(), &wallet, &mint).is_err());
        assert!(confirmed_balance_deltas(&tx, &sig, &Pubkey::new_unique(), &mint).is_err());
        assert!(confirmed_balance_deltas(&tx, &sig, &wallet, &Pubkey::new_unique()).is_err());
        tx.transaction.meta.as_mut().unwrap().status =
            Err(solana_sdk::transaction::TransactionError::AccountNotFound);
        assert!(confirmed_balance_deltas(&tx, &sig, &wallet, &mint).is_err());
        tx.transaction.meta = None;
        assert!(confirmed_balance_deltas(&tx, &sig, &wallet, &mint).is_err());
    }

    #[test]
    fn observes_account_creation_and_closure_only_with_corresponding_zero_lamports() {
        let (tx, sig, wallet, mint) = fixture();
        let copy = || -> EncodedConfirmedTransactionWithStatusMeta {
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap()
        };
        let mut created = copy();
        let meta = created.transaction.meta.as_mut().unwrap();
        if let OptionSerializer::Some(rows) = &mut meta.pre_token_balances {
            let index = usize::from(rows.remove(0).account_index);
            meta.pre_balances[index] = 0;
        }
        let created_delta = confirmed_balance_deltas(&created, &sig, &wallet, &mint).unwrap();
        assert_eq!(created_delta.token_delta_raw, 120);
        assert_eq!(created_delta.inferred_zero_token_rows, 1);
        let mut closed = copy();
        let meta = closed.transaction.meta.as_mut().unwrap();
        if let OptionSerializer::Some(rows) = &mut meta.post_token_balances {
            let index = usize::from(rows.remove(0).account_index);
            meta.post_balances[index] = 0;
        }
        let closed_delta = confirmed_balance_deltas(&closed, &sig, &wallet, &mint).unwrap();
        assert_eq!(closed_delta.token_delta_raw, -110);
        assert_eq!(closed_delta.inferred_zero_token_rows, 1);
        let mut missing = copy();
        if let OptionSerializer::Some(rows) = &mut missing
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .pre_token_balances
        {
            rows.remove(0);
        }
        assert!(confirmed_balance_deltas(&missing, &sig, &wallet, &mint).is_err());
        let mut ambiguous = copy();
        let meta = ambiguous.transaction.meta.as_mut().unwrap();
        if let OptionSerializer::Some(rows) = &mut meta.pre_token_balances {
            let index = usize::from(rows.remove(0).account_index);
            meta.pre_balances[index] = 0;
            meta.post_balances[index] = 0;
        }
        assert!(confirmed_balance_deltas(&ambiguous, &sig, &wallet, &mint).is_err());
    }

    #[test]
    fn observes_negative_delta_and_refuses_account_identity_changes() {
        let (tx, sig, wallet, mint) = fixture();
        let copy = || -> EncodedConfirmedTransactionWithStatusMeta {
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap()
        };
        let mut sell = copy();
        if let OptionSerializer::Some(rows) =
            &mut sell.transaction.meta.as_mut().unwrap().post_token_balances
        {
            rows[0].ui_token_amount.amount = "20".into();
            rows[1].ui_token_amount.amount = "40".into();
        }
        assert_eq!(
            confirmed_balance_deltas(&sell, &sig, &wallet, &mint)
                .unwrap()
                .token_delta_raw,
            -90
        );
        let mut changed_owner = copy();
        if let OptionSerializer::Some(rows) = &mut changed_owner
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].owner = OptionSerializer::Some(Pubkey::new_unique().to_string());
        }
        assert!(confirmed_balance_deltas(&changed_owner, &sig, &wallet, &mint).is_err());
        let mut changed_mint = copy();
        if let OptionSerializer::Some(rows) = &mut changed_mint
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].mint = Pubkey::new_unique().to_string();
        }
        assert!(confirmed_balance_deltas(&changed_mint, &sig, &wallet, &mint).is_err());
        let mut bad_index = copy();
        if let OptionSerializer::Some(rows) = &mut bad_index
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].account_index = u8::MAX;
        }
        assert!(confirmed_balance_deltas(&bad_index, &sig, &wallet, &mint).is_err());
    }

    #[test]
    fn refuses_incomplete_ambiguous_and_inconsistent_metadata() {
        let (tx, sig, wallet, mint) = fixture();
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .pre_balances
            .clear();
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances = OptionSerializer::None;
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        if let OptionSerializer::Some(rows) = &mut broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows.pop();
        }
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        if let OptionSerializer::Some(rows) = &mut broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].owner = OptionSerializer::None;
        }
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        if let OptionSerializer::Some(rows) = &mut broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].ui_token_amount.decimals = 7;
        }
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        if let OptionSerializer::Some(rows) = &mut broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows.push(rows[0].clone());
        }
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
        let mut broken: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::to_value(&tx).unwrap()).unwrap();
        if let OptionSerializer::Some(rows) = &mut broken
            .transaction
            .meta
            .as_mut()
            .unwrap()
            .post_token_balances
        {
            rows[0].ui_token_amount.amount = "NaN".into();
        }
        assert!(confirmed_balance_deltas(&broken, &sig, &wallet, &mint).is_err());
    }
}
