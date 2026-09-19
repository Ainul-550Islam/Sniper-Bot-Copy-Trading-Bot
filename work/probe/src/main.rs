use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::*;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_request::RpcRequest;
use solana_client::rpc_response::{Response, RpcContactInfo, RpcVersionInfo};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use solana_sdk::message::v0::{self, MessageAddressTableLookup};
use solana_sdk::message::{Message, MessageHeader, VersionedMessage};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::hash::Hash;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::system_instruction;
use std::str::FromStr;

#[allow(dead_code)]
async fn probe() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = RpcClient::new_with_commitment("https://api.mainnet-beta.solana.com".to_string(), CommitmentConfig::processed());
    let kp = Keypair::new();
    let pk = Pubkey::from_str("11111111111111111111111111111111")?;

    let _: u64 = rpc.get_balance(&pk).await?;
    let _: solana_sdk::account::Account = rpc.get_account(&pk).await?;
    let _: Vec<Option<solana_sdk::account::Account>> = rpc.get_multiple_accounts(&[pk, pk]).await?;
    let _: Vec<u8> = rpc.get_account_data(&pk).await?;
    let _: Option<solana_sdk::account::Account> = rpc.get_account(&pk).await.ok();
    let bh: (Hash, u64) = rpc.get_latest_blockhash_with_commitment(CommitmentConfig::confirmed()).await?;
    let _: Hash = bh.0;
    let _: u64 = bh.1;
    let _: RpcVersionInfo = rpc.get_version().await?;
    let _: u64 = rpc.get_slot().await?;
    let sigs = rpc.get_signatures_for_address_with_config(
        &pk,
        GetConfirmedSignaturesForAddress2Config { before: None, until: None, limit: Some(10), commitment: None },
    ).await?;
    let _: Vec<Signature> = sigs.iter().map(|s| Signature::from_str(&s.signature).unwrap()).collect();
    let _: u64 = sigs[0].slot;
    let _: Option<String> = sigs[0].err.as_ref().map(|e| e.to_string());
    let _: Option<i64> = sigs[0].block_time;
    let _: solana_transaction_status::UiConfirmedBlock = rpc.get_block_with_config(
        1, RpcBlockConfig { encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64), transaction_details: Some(solana_transaction_status::TransactionDetails::Full), rewards: Some(false), commitment: None, max_supported_transaction_version: Some(0) },
    ).await?;

    let ix = Instruction { program_id: pk, accounts: vec![AccountMeta::new(pk, false)], data: vec![9u8,1,2] };
    // v0 message compile
    let msg = v0::Message::try_compile(&kp.pubkey(), &[ix.clone()], &[], bh.0)?;
    let vtx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[&kp])?;
    let bytes = bincode::serialize(&vtx)?;
    let _: usize = bytes.len();

    // legacy message
    let _legacy = Message::new_with_blockhash(&[ix.clone()], Some(&kp.pubkey()), &bh.0);
    let _hdr = MessageHeader { num_required_signatures: 1, num_readonly_signed_accounts: 0, num_readonly_unsigned_accounts: 1 };
    let _lookup = MessageAddressTableLookup { account_key: pk, writable_indexes: vec![0u8], readonly_indexes: vec![1u8] };

    // compute budget + system
    let _cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let _p = ComputeBudgetInstruction::set_compute_unit_price(1_000);
    let _transfer = system_instruction::transfer(&kp.pubkey(), &pk, 1);
    let _create = system_instruction::create_account(&kp.pubkey(), &pk, 1, 1, &pk);

    // ATA
    let _ata = spl_associated_token_account::get_associated_token_address(&pk, &pk);
    let _ata_ix = spl_associated_token_account::instruction::create_associated_token_account_idempotent(&kp.pubkey(), &pk, &pk, &pk);
    let _tok_ix = spl_token::instruction::close_account(&pk, &pk, &pk, &pk, &[])?;
    let _tok_tf = spl_token::instruction::transfer_checked(&pk, &pk, &pk, &pk, &pk, &[], 1, 9)?;
    let _tok_mint = spl_token::instruction::mint_to(&pk, &pk, &pk, &pk, &[], 1)?;
    let _sync = spl_token::instruction::sync_native(&pk, &pk)?;

    // simulate + send
    let sim_cfg = RpcSimulateTransactionConfig { sig_verify: false, replace_recent_blockhash: true, commitment: Some(CommitmentConfig::processed()), encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64), accounts: None, min_context_slot: None, inner_instructions: false };
    let _: Response<solana_client::rpc_response::RpcSimulateTransactionResult> = rpc.simulate_transaction_with_config(&vtx, sim_cfg).await?;
    let send_cfg = RpcSendTransactionConfig { skip_preflight: true, preflight_commitment: Some(solana_sdk::commitment_config::CommitmentLevel::Processed), encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64), max_retries: Some(0), min_context_slot: None };
    let _: Signature = rpc.send_transaction_with_config(&vtx, send_cfg).await?;
    let _: Signature = rpc.send_transaction_with_config(&vtx, send_cfg).await?;
    let _: Signature = rpc.send_transaction(&vtx).await?;
    let tx_cfg = RpcTransactionConfig { encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64), commitment: Some(CommitmentConfig::confirmed()), max_supported_transaction_version: Some(0) };
    let tx: Option<solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta> =
        rpc.get_transaction_with_config(&Signature::default(), tx_cfg).await.ok();
    if let Some(t) = tx {
        let _: u64 = t.slot;
        let _: Option<i64> = t.block_time;
        let meta: Option<solana_transaction_status::UiTransactionStatusMeta> = t.transaction.meta;
        if let Some(m) = meta {
            let _: Option<String> = m.err.map(|e| e.to_string());
            let _ = &m.log_messages;
            let _: u64 = m.fee;
        }
        match &t.transaction.transaction {
            solana_transaction_status::EncodedTransaction::LegacyBinary(b58) => {
                let raw = bs58::decode(b58).into_vec().unwrap_or_default();
                let _decoded: Option<VersionedTransaction> = bincode::deserialize(&raw).ok();
            }
            solana_transaction_status::EncodedTransaction::Binary(b64, enc) => {
                let raw = match enc {
                    solana_transaction_status::TransactionBinaryEncoding::Base64 => {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default()
                    }
                    _ => bs58::decode(b64).into_vec().unwrap_or_default(),
                };
                let _decoded: Option<VersionedTransaction> = bincode::deserialize(&raw).ok();
            }
            solana_transaction_status::EncodedTransaction::Json(_)
            | solana_transaction_status::EncodedTransaction::Accounts(_) => {}
        }
    }

    // raw jsonrpc
    let params = serde_json::json!([pk.to_string(), {"encoding":"base64","commitment":"processed"}]);
    let _: serde_json::Value = rpc.send(RpcRequest::GetAccountInfo, params).await?;
    let _: serde_json::Value = rpc.send(RpcRequest::Custom { method: "getAsset" }, serde_json::json!([])).await?;
    let _: serde_json::Value = rpc.send(RpcRequest::GetTokenAccountsByOwner, serde_json::json!([pk.to_string(), {"mint": pk.to_string()}, {"encoding":"jsonParsed"}])).await?;


    // ws url
    let _: String = rpc.url();
    let _ws: Option<String> = Some(rpc.url());
    let _ = std::marker::PhantomData::<RpcContactInfo>;
    Ok(())
}


#[test]
fn tags() {
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let cp = ComputeBudgetInstruction::set_compute_unit_price(1_000);
    println!("COMPUTE_BUDGET_PROGRAM = {}", cu.program_id);
    println!("set_compute_unit_limit: tag={} data={:?}", cu.data[0], cu.data);
    println!("set_compute_unit_price: tag={} data={:?}", cp.data[0], cp.data);
    let sync = spl_token::instruction::sync_native(&spl_token::id(), &Pubkey::new_unique()).unwrap();
    println!("sync_native: program={} tag={} data={:?}", sync.program_id, sync.data[0], sync.data);
    let close = spl_token::instruction::close_account(&spl_token::id(), &Pubkey::new_unique(), &Pubkey::new_unique(), &Pubkey::new_unique(), &[]).unwrap();
    println!("close_account: tag={}", close.data[0]);
    let ata = spl_associated_token_account::instruction::create_associated_token_account_idempotent(&Pubkey::new_unique(), &Pubkey::new_unique(), &Pubkey::new_unique(), &spl_token::id());
    println!("ata idempotent: program={} accounts={} data={:?}", ata.program_id, ata.accounts.len(), ata.data);
    let xfer = system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1);
    println!("system transfer: program={} tag bytes={:?}", xfer.program_id, &xfer.data[..4]);
}
fn main() { println!("probe compiled"); }

#[cfg(test)]
mod ray_bump {
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;
    #[test]
    fn print_bump() {
        let prog = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
        let (addr, bump) = Pubkey::find_program_address(&[b"amm authority"], &prog);
        println!("RAY_AMM_AUTHORITY={} bump={}", addr, bump);
        // Also compute the market vault signer seed check for nonce bytes.
    }
}
