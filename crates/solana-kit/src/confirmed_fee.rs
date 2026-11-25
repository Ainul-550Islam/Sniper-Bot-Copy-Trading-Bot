//! Conservative verification of a direct native-SOL fee transfer in an
//! RPC-confirmed transaction. This is not a venue-fill decoder or ledger.
//! RPC data and commitment remain trust assumptions; transaction signature
//! and the actual message are checked locally before recognizing the leg.

use bot_core::error::{BotError, BotResult};
use solana_sdk::{message::VersionedMessage, pubkey::Pubkey, signature::Signature};
use solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta;

/// A structurally verified direct fee leg, in exact lamports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedDirectFee {
    pub signature: Signature,
    pub slot: u64,
    pub payer: Pubkey,
    pub vault: Pubkey,
    pub lamports: u64,
}

/// Confirm exactly one top-level System Program transfer of `lamports` from
/// the signer `payer` to `vault` in a successful transaction returned by RPC.
/// Address-lookup messages are refused because this function does not resolve
/// loaded addresses; inner instructions and Jupiter token fees are out of scope.
/// This proves neither the swap's economics nor the provider's honesty.
pub fn verify_confirmed_direct_fee(
    response: &EncodedConfirmedTransactionWithStatusMeta,
    signature: &Signature,
    payer: &Pubkey,
    vault: &Pubkey,
    lamports: u64,
) -> BotResult<ConfirmedDirectFee> {
    let invalid = |message: &str| BotError::invalid(message);
    if lamports == 0 || payer == vault {
        return Err(invalid("invalid expected direct fee"));
    }
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
    if tx.signatures.first() != Some(signature) {
        return Err(invalid(
            "transaction signature does not match the expected signature",
        ));
    }
    tx.verify_and_hash_message()
        .map_err(|_| invalid("invalid transaction signature"))?;
    if let VersionedMessage::V0(message) = &tx.message {
        if !message.address_table_lookups.is_empty() {
            return Err(invalid(
                "lookup-table transaction is not supported by direct fee verifier",
            ));
        }
    }
    let keys = tx.message.static_account_keys();
    let signer_count = usize::from(tx.message.header().num_required_signatures);
    let mut matches = 0usize;
    for ix in tx.message.instructions() {
        if keys.get(ix.program_id_index as usize) != Some(&solana_sdk::system_program::id())
            || ix.data.len() != 12
            || ix.data[..4] != 2u32.to_le_bytes()
            || ix.accounts.len() != 2
        {
            continue;
        }
        let payer_index = usize::from(ix.accounts[0]);
        let vault_index = usize::from(ix.accounts[1]);
        if payer_index >= signer_count
            || keys.get(payer_index) != Some(payer)
            || keys.get(vault_index) != Some(vault)
        {
            continue;
        }
        let amount = u64::from_le_bytes(ix.data[4..12].try_into().expect("length checked"));
        if amount != lamports {
            return Err(invalid("direct fee transfer amount does not match"));
        }
        matches += 1;
    }
    if matches != 1 {
        return Err(invalid("expected exactly one matching direct fee transfer"));
    }
    Ok(ConfirmedDirectFee {
        signature: *signature,
        slot: response.slot,
        payer: *payer,
        vault: *vault,
        lamports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signer},
        transaction::Transaction,
    };

    fn response(
        payer: &Keypair,
        vault: &Pubkey,
        count: usize,
        amount: u64,
    ) -> (EncodedConfirmedTransactionWithStatusMeta, Signature) {
        let instructions = (0..count)
            .map(|_| solana_system_interface::instruction::transfer(&payer.pubkey(), vault, amount))
            .collect::<Vec<_>>();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[payer],
            Hash::new_unique(),
        );
        let signature = tx.signatures[0];
        let bytes = bincode::serialize(&tx).unwrap();
        let tx = serde_json::from_value(serde_json::json!({
            "slot": 55, "transaction": [base64::engine::general_purpose::STANDARD.encode(bytes), "base64"],
            "meta": {"err": null, "status": {"Ok": null}, "fee": 5000, "preBalances": [], "postBalances": []},
            "blockTime": null
        })).unwrap();
        (tx, signature)
    }

    #[test]
    fn verifies_exact_direct_transfer_and_rejects_mismatch() {
        let payer = Keypair::new();
        let vault = Pubkey::new_unique();
        let (tx, signature) = response(&payer, &vault, 1, 12_345);
        let verified =
            verify_confirmed_direct_fee(&tx, &signature, &payer.pubkey(), &vault, 12_345).unwrap();
        assert_eq!((verified.lamports, verified.slot), (12_345, 55));
        assert!(verify_confirmed_direct_fee(
            &tx,
            &Signature::default(),
            &payer.pubkey(),
            &vault,
            12_345
        )
        .is_err());
        assert!(
            verify_confirmed_direct_fee(&tx, &signature, &payer.pubkey(), &vault, 12_346).is_err()
        );
        assert!(verify_confirmed_direct_fee(
            &tx,
            &signature,
            &payer.pubkey(),
            &Pubkey::new_unique(),
            12_345
        )
        .is_err());
        let (duplicate, signature) = response(&payer, &vault, 2, 12_345);
        assert!(verify_confirmed_direct_fee(
            &duplicate,
            &signature,
            &payer.pubkey(),
            &vault,
            12_345
        )
        .is_err());
    }

    #[test]
    fn rejects_tampered_transaction_even_when_primary_signature_is_unchanged() {
        use solana_transaction_status::{EncodedTransaction, TransactionBinaryEncoding};
        let payer = Keypair::new();
        let vault = Pubkey::new_unique();
        let (mut response, signature) = response(&payer, &vault, 1, 100);
        let mut transaction = response.transaction.transaction.decode().unwrap();
        match &mut transaction.message {
            VersionedMessage::Legacy(message) => message.instructions[0].data[4] ^= 1,
            VersionedMessage::V0(_) => panic!("fixture must use a legacy message"),
        }
        response.transaction.transaction = EncodedTransaction::Binary(
            base64::engine::general_purpose::STANDARD
                .encode(bincode::serialize(&transaction).unwrap()),
            TransactionBinaryEncoding::Base64,
        );
        assert!(
            verify_confirmed_direct_fee(&response, &signature, &payer.pubkey(), &vault, 101)
                .is_err()
        );
    }

    #[test]
    fn refuses_missing_or_failed_execution_metadata() {
        let payer = Keypair::new();
        let vault = Pubkey::new_unique();
        let (mut tx, signature) = response(&payer, &vault, 1, 100);
        let saved = tx.transaction.meta.take();
        assert!(
            verify_confirmed_direct_fee(&tx, &signature, &payer.pubkey(), &vault, 100).is_err()
        );
        tx.transaction.meta = saved;
        tx.transaction.meta.as_mut().unwrap().status =
            Err(solana_sdk::transaction::TransactionError::AccountNotFound);
        assert!(
            verify_confirmed_direct_fee(&tx, &signature, &payer.pubkey(), &vault, 100).is_err()
        );
    }
}
