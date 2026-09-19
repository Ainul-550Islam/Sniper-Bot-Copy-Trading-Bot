//! Transaction assembly: compute budget, Jito tip, wSOL handling, address
//! lookup tables and v0 message compilation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use solana_sdk::address_lookup_table::AddressLookupTableAccount;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::v0::{Message as MessageV0, MessageAddressTableLookup};
use solana_sdk::message::{MessageHeader, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use solana_system_interface::instruction as system_instruction;
use tracing::debug;

use bot_core::error::{BotError, BotResult, SignerError};
use bot_core::maths;

use crate::consts::*;
use crate::rpc::Rpc;
use crate::signer::SignerRegistry;
use crate::tokens::Wallet;

/// The 1232-byte ceiling every Solana transaction must fit in.
pub const PACKET_DATA_SIZE: usize = 1232;

/// What to build.
#[derive(Debug, Clone)]
pub struct TxRequest {
    /// The trading instructions, in execution order.
    pub instructions: Vec<Instruction>,
    /// Prepend `set_compute_unit_limit` / `set_compute_unit_price`.
    pub compute_unit_limit: u32,
    pub priority_fee_micro_lamports: u64,
    /// Add a transfer to a random Jito tip account (MEV protection).
    pub jito_tip_lamports: u64,
    /// Wrap this many lamports into the wSOL ATA first.
    pub wrap_sol_lamports: u64,
    /// Close the wSOL ATA at the end (sweeps dust back to SOL).
    pub unwrap_sol: bool,
    /// Address lookup tables to compress the account list with.
    pub lookup_tables: Vec<AddressLookupTableAccount>,
    /// Override the blockhash (used when re-signing a prebuilt transaction).
    pub blockhash: Option<Hash>,
    /// Extra signers required by the instructions, beyond the wallet.
    ///
    /// Every public key listed here MUST (a) appear in the compiled message's
    /// required-signer set and (b) be resolvable through the builder's
    /// [`SignerRegistry`]. Conversely, every required signer that is not the
    /// wallet must be listed here. Duplicates are collapsed and signed once;
    /// violations fail the build with a structured [`SignerError`] — a
    /// required signer is never silently ignored.
    pub extra_signers: Vec<Pubkey>,
    /// Human label used in logs and events.
    pub label: String,
}

impl Default for TxRequest {
    fn default() -> Self {
        TxRequest {
            instructions: Vec::new(),
            compute_unit_limit: 400_000,
            priority_fee_micro_lamports: 0,
            jito_tip_lamports: 0,
            wrap_sol_lamports: 0,
            unwrap_sol: false,
            lookup_tables: Vec::new(),
            blockhash: None,
            extra_signers: Vec::new(),
            label: "tx".into(),
        }
    }
}

impl TxRequest {
    pub fn new(label: impl Into<String>) -> Self {
        TxRequest {
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn with_instruction(mut self, ix: Instruction) -> Self {
        self.instructions.push(ix);
        self
    }

    pub fn with_instructions(mut self, ixs: impl IntoIterator<Item = Instruction>) -> Self {
        self.instructions.extend(ixs);
        self
    }

    pub fn priority_fee(mut self, micro_lamports: u64) -> Self {
        self.priority_fee_micro_lamports = micro_lamports;
        self
    }

    pub fn compute_units(mut self, limit: u32) -> Self {
        self.compute_unit_limit = limit;
        self
    }

    pub fn jito_tip(mut self, lamports: u64) -> Self {
        self.jito_tip_lamports = lamports;
        self
    }

    pub fn wrap_sol(mut self, lamports: u64) -> Self {
        self.wrap_sol_lamports = lamports;
        self
    }

    pub fn lookup_table(mut self, table: AddressLookupTableAccount) -> Self {
        self.lookup_tables.push(table);
        self
    }
}

/// Builds and signs transactions for one wallet, plus any additional signers
/// resolved through an optional [`SignerRegistry`].
pub struct TxBuilder<'a> {
    rpc: &'a Rpc,
    wallet: &'a Wallet,
    registry: Option<Arc<SignerRegistry>>,
}

impl<'a> TxBuilder<'a> {
    /// Wallet-only builder. Requests carrying `extra_signers` (or messages
    /// whose instructions demand a second signer) fail with an explicit
    /// [`SignerError::MissingSigner`] — never with a silently short signature
    /// set.
    pub fn new(rpc: &'a Rpc, wallet: &'a Wallet) -> Self {
        TxBuilder {
            rpc,
            wallet,
            registry: None,
        }
    }

    /// Builder with multi-signer support: additional required signers are
    /// resolved by public key through the registry.
    pub fn with_registry(rpc: &'a Rpc, wallet: &'a Wallet, registry: Arc<SignerRegistry>) -> Self {
        TxBuilder {
            rpc,
            wallet,
            registry: Some(registry),
        }
    }

    /// Assemble the final instruction list for a request, including the
    /// compute-budget, wSOL and Jito tip instructions.
    pub async fn prepare(&self, req: &TxRequest) -> BotResult<Vec<Instruction>> {
        let mut ixs: Vec<Instruction> = Vec::with_capacity(req.instructions.len() + 5);

        // 1. Compute budget first. These must appear in the transaction, and
        //    putting them ahead of the swap is the convention every router
        //    follows.
        ixs.extend(Wallet::budget_instructions(
            req.compute_unit_limit,
            req.priority_fee_micro_lamports,
        ));

        // 2. Wrap SOL if the trade needs wSOL as input (Raydium/Jupiter paths).
        if req.wrap_sol_lamports > 0 {
            let (_ata, wrap_ixs) = self
                .wallet
                .wrap_sol_instructions(self.rpc, req.wrap_sol_lamports)
                .await?;
            ixs.extend(wrap_ixs);
        }

        // 3. The actual trading instructions.
        ixs.extend(req.instructions.iter().cloned());

        // 4. Sweep wSOL back to SOL.
        if req.unwrap_sol {
            let unwrap_ixs = self.wallet.unwrap_sol_instructions(self.rpc).await?;
            ixs.extend(unwrap_ixs);
        }

        // 5. Jito tip last: block engines require the tip transfer to be part
        //    of the bundle and it must not fail before the swap executes.
        if req.jito_tip_lamports > 0 {
            ixs.push(jito_tip_instruction(
                self.wallet.pubkey,
                req.jito_tip_lamports,
            ));
        }

        if ixs.is_empty() {
            return Err(BotError::invalid("transaction has no instructions"));
        }
        Ok(ixs)
    }

    /// Build, size-check and sign. Returns the signed transaction plus its
    /// serialized size (useful for the latency budget).
    pub async fn build(&self, req: &TxRequest) -> BotResult<BuiltTx> {
        let ixs = self.prepare(req).await?;
        let blockhash = match req.blockhash {
            Some(bh) => bh,
            None => self.rpc.latest_blockhash(false).await?.blockhash,
        };

        let message = if req.lookup_tables.is_empty() {
            // Prefer a v0 message with no lookups: identical account capacity to
            // legacy for our sizes, but it keeps the door open for ALTs later
            // and matches what most modern routers send.
            let msg = MessageV0::try_compile(&self.wallet.pubkey, &ixs, &[], blockhash)
                .map_err(|e| BotError::solana(format!("compile {}: {e}", req.label)))?;
            VersionedMessage::V0(msg)
        } else {
            let msg =
                MessageV0::try_compile(&self.wallet.pubkey, &ixs, &req.lookup_tables, blockhash)
                    .map_err(|e| {
                        BotError::solana(format!("compile {} with ALT: {e}", req.label))
                    })?;
            VersionedMessage::V0(msg)
        };

        // --- multi-signer assembly ----------------------------------------
        //
        // The compiled message is authoritative: its first
        // `num_required_signatures` account keys must each produce a
        // signature, in that order. The wallet signs its own slot locally;
        // every other required key must be (a) declared in
        // `req.extra_signers` and (b) resolvable through the registry.
        let msg_bytes = message.serialize();
        let required = required_signer_keys(&message);

        // Validate the caller's declaration against the message before
        // signing anything: exact set equality (minus the wallet).
        let mut declared: HashSet<Pubkey> = HashSet::new();
        for pk in &req.extra_signers {
            declared.insert(*pk); // duplicates collapse; signed once below
        }
        let needed: HashSet<Pubkey> = required
            .iter()
            .copied()
            .filter(|k| *k != self.wallet.pubkey)
            .collect();
        if let Some(pk) = declared.difference(&needed).next() {
            return Err(BotError::Signer(SignerError::ExtraSignerNotRequired {
                pubkey: pk.to_string(),
            }));
        }
        if let Some(pk) = needed.difference(&declared).next() {
            return Err(BotError::Signer(SignerError::SignerMismatch {
                expected: pk.to_string(),
                found: "not declared in extra_signers".to_string(),
            }));
        }
        for pk in &needed {
            let registry = self.registry.as_ref().ok_or_else(|| {
                BotError::Signer(SignerError::MissingSigner {
                    pubkey: pk.to_string(),
                })
            })?;
            if registry.find_by_pubkey(pk).is_none() {
                return Err(BotError::Signer(SignerError::MissingSigner {
                    pubkey: pk.to_string(),
                }));
            }
        }

        // Sign in message order.
        let mut signatures: Vec<Signature> = Vec::with_capacity(required.len());
        for key in &required {
            if key == &self.wallet.pubkey {
                signatures.push(self.wallet.sign_message_sync(&msg_bytes));
            } else {
                let signer = self
                    .registry
                    .as_ref()
                    .and_then(|r| r.find_by_pubkey(key))
                    .ok_or_else(|| {
                        BotError::Signer(SignerError::MissingSigner {
                            pubkey: key.to_string(),
                        })
                    })?;
                let sig = signer.sign_message(&msg_bytes).await.map_err(|e| {
                    BotError::Signer(SignerError::SigningFailed {
                        context: format!("{}: signer {key}: {e}", req.label),
                    })
                })?;
                signatures.push(sig);
            }
        }

        let tx = VersionedTransaction {
            message,
            signatures,
        };

        let bytes = bincode::serialize(&tx)
            .map_err(|e| BotError::encoding(format!("serialize {}: {e}", req.label)))?;
        let size = bytes.len();
        if size > PACKET_DATA_SIZE {
            return Err(BotError::solana(format!(
                "{} is {size} bytes, over the {PACKET_DATA_SIZE} byte limit — \
                 reduce accounts or use an address lookup table",
                req.label
            )));
        }

        let account_count = match &tx.message {
            VersionedMessage::V0(m) => {
                m.account_keys.len()
                    + m.address_table_lookups
                        .iter()
                        .map(|l| l.writable_indexes.len() + l.readonly_indexes.len())
                        .sum::<usize>()
            }
            VersionedMessage::Legacy(m) => m.account_keys.len(),
        };

        debug!(
            label = %req.label,
            size,
            account_count,
            instructions = ixs.len(),
            "built transaction"
        );

        Ok(BuiltTx {
            tx,
            bytes,
            size,
            account_count,
            blockhash,
            label: req.label.clone(),
            instructions: ixs,
        })
    }
}

/// A signed, serialized transaction ready to simulate or broadcast.
#[derive(Debug, Clone)]
pub struct BuiltTx {
    pub tx: VersionedTransaction,
    pub bytes: Vec<u8>,
    pub size: usize,
    pub account_count: usize,
    pub blockhash: Hash,
    pub label: String,
    pub instructions: Vec<Instruction>,
}

impl BuiltTx {
    pub fn signature(&self) -> Signature {
        self.tx.signatures.first().copied().unwrap_or_default()
    }

    /// How much headroom is left before the packet limit.
    pub fn size_headroom(&self) -> usize {
        PACKET_DATA_SIZE.saturating_sub(self.size)
    }
}

/// The public keys that must sign `message`, in the exact order the runtime
/// expects signatures (the first `num_required_signatures` static account
/// keys). Works for both v0 and legacy messages.
pub fn required_signer_keys(message: &VersionedMessage) -> Vec<Pubkey> {
    let (header, keys) = match message {
        VersionedMessage::V0(m) => (m.header, &m.account_keys),
        VersionedMessage::Legacy(m) => (m.header, &m.account_keys),
    };
    keys.iter()
        .take(header.num_required_signatures as usize)
        .copied()
        .collect()
}

/// A transfer to one of Jito's eight tip accounts, picked at random per
/// transaction so tips spread across the fleet.
pub fn jito_tip_instruction(payer: Pubkey, lamports: u64) -> Instruction {
    let accounts = JITO_TIP_ACCOUNTS.clone();
    let idx = rand::random::<usize>() % accounts.len();
    system_instruction::transfer(&payer, &accounts[idx], lamports)
}

/// Load an address lookup table account from the chain.
pub async fn fetch_lookup_table(
    rpc: &Rpc,
    address: &Pubkey,
) -> BotResult<AddressLookupTableAccount> {
    let account = rpc
        .get_account(address)
        .await?
        .ok_or_else(|| BotError::solana(format!("lookup table {address} not found")))?;
    let table =
        solana_sdk::address_lookup_table::state::AddressLookupTable::deserialize(&account.data)
            .map_err(|e| BotError::solana(format!("decode lookup table {address}: {e}")))?;
    Ok(AddressLookupTableAccount {
        key: *address,
        addresses: table.addresses.to_vec(),
    })
}

// --------------------------------------------------------------------------
// Manual v0 message construction
// --------------------------------------------------------------------------
//
// `MessageV0::try_compile` is the right default, but a sniper sometimes wants
// to hand-roll the message: to control exact account ordering, or to reuse a
// precomputed account list learned from a working transaction. These helpers do
// the key ordering and deduplication the runtime requires.

/// Build a v0 message by hand.
///
/// `keys` must be in the order the program expects; this function re-orders
/// them into the runtime's canonical layout (writable signers, readonly
/// signers, writable non-signers, readonly non-signers, fee payer first) and
/// remaps the instruction account indexes accordingly.
pub fn compile_v0_manual(
    payer: Pubkey,
    blockhash: Hash,
    instructions: &[Instruction],
    keys: Vec<AccountMeta>,
    lookups: Vec<MessageAddressTableLookup>,
) -> BotResult<MessageV0> {
    if instructions.is_empty() {
        return Err(BotError::invalid("no instructions"));
    }

    // Deduplicate while preserving the first-seen flags (the runtime takes the
    // union of permissions, so a key that is writable anywhere is writable).
    let mut merged: Vec<AccountMeta> = Vec::with_capacity(keys.len() + 1);
    let mut index_of: HashMap<Pubkey, usize> = HashMap::with_capacity(keys.len() + 1);

    // The fee payer is always index 0 and always a writable signer.
    index_of.insert(payer, 0);
    merged.push(AccountMeta::new(payer, true));

    for meta in keys {
        match index_of.get(&meta.pubkey) {
            Some(&i) => {
                let existing = &mut merged[i];
                existing.is_writable |= meta.is_writable;
                existing.is_signer |= meta.is_signer;
            }
            None => {
                index_of.insert(meta.pubkey, merged.len());
                merged.push(meta);
            }
        }
    }

    // Program ids must be static keys and cannot come from a lookup table.
    for ix in instructions {
        if let std::collections::hash_map::Entry::Vacant(e) = index_of.entry(ix.program_id) {
            e.insert(merged.len());
            merged.push(AccountMeta::new_readonly(ix.program_id, false));
        }
    }

    // Canonical ordering: [writable signers][readonly signers]
    //                   [writable non-signers][readonly non-signers]
    let mut writable_signers: Vec<AccountMeta> = Vec::new();
    let mut readonly_signers: Vec<AccountMeta> = Vec::new();
    let mut writable_non: Vec<AccountMeta> = Vec::new();
    let mut readonly_non: Vec<AccountMeta> = Vec::new();

    for (i, meta) in merged.into_iter().enumerate() {
        if i == 0 {
            // Keep the fee payer pinned at index 0.
            writable_signers.push(AccountMeta::new(payer, true));
            continue;
        }
        match (meta.is_signer, meta.is_writable) {
            (true, true) => writable_signers.push(meta),
            (true, false) => readonly_signers.push(meta),
            (false, true) => writable_non.push(meta),
            (false, false) => readonly_non.push(meta),
        }
    }

    let mut account_keys: Vec<Pubkey> = Vec::new();
    let mut remap: HashMap<Pubkey, u8> = HashMap::new();
    let push = |list: Vec<AccountMeta>,
                account_keys: &mut Vec<Pubkey>,
                remap: &mut HashMap<Pubkey, u8>| {
        for meta in list {
            if remap.contains_key(&meta.pubkey) {
                continue;
            }
            let idx = u8::try_from(account_keys.len())
                .map_err(|_| BotError::solana("more than 255 static account keys"))?;
            remap.insert(meta.pubkey, idx);
            account_keys.push(meta.pubkey);
        }
        Ok::<(), BotError>(())
    };
    // The canonical buckets are disjoint (every key lands in exactly one), so
    // the header counts are just their lengths. Capture them before `push`
    // consumes the vectors.
    let n_writable_signers = writable_signers.len();
    let n_readonly_signers = readonly_signers.len();
    let n_readonly_non = readonly_non.len();

    push(writable_signers, &mut account_keys, &mut remap)?;
    push(readonly_signers, &mut account_keys, &mut remap)?;
    push(writable_non, &mut account_keys, &mut remap)?;
    push(readonly_non, &mut account_keys, &mut remap)?;

    let header = MessageHeader {
        num_required_signatures: u8::try_from(n_writable_signers + n_readonly_signers)
            .map_err(|_| BotError::solana("too many signers"))?,
        num_readonly_signed_accounts: u8::try_from(n_readonly_signers)
            .map_err(|_| BotError::solana("too many readonly signers"))?,
        num_readonly_unsigned_accounts: u8::try_from(n_readonly_non)
            .map_err(|_| BotError::solana("too many readonly accounts"))?,
    };

    let compiled = instructions
        .iter()
        .map(|ix| {
            let program_id_index = *remap
                .get(&ix.program_id)
                .ok_or_else(|| BotError::solana("program id missing from static keys"))?;
            let accounts = ix
                .accounts
                .iter()
                .map(|meta| {
                    remap.get(&meta.pubkey).copied().ok_or_else(|| {
                        BotError::solana(format!(
                            "account {} is not in the static keys and no lookup was supplied",
                            meta.pubkey
                        ))
                    })
                })
                .collect::<BotResult<Vec<u8>>>()?;
            Ok(solana_sdk::instruction::CompiledInstruction {
                program_id_index,
                accounts,
                data: ix.data.clone(),
            })
        })
        .collect::<BotResult<Vec<_>>>()?;

    Ok(MessageV0 {
        header,
        account_keys,
        recent_blockhash: blockhash,
        instructions: compiled,
        address_table_lookups: lookups,
    })
}

/// SOL amount formatting helper used by logs and Telegram alerts.
pub fn fmt_sol(lamports: u64) -> String {
    format!("{:.6} SOL", maths::lamports_to_sol(lamports))
}

/// Convenience: an AccountMeta list from (pubkey, writable, signer) triples.
pub fn metas(items: Vec<(Pubkey, bool, bool)>) -> Vec<AccountMeta> {
    items
        .into_iter()
        .map(|(pk, w, s)| match (w, s) {
            (true, true) => AccountMeta::new(pk, true),
            (true, false) => AccountMeta::new(pk, false),
            (false, true) => AccountMeta::new_readonly(pk, true),
            (false, false) => AccountMeta::new_readonly(pk, false),
        })
        .collect()
}

/// Serialization for the dashboard: a compact view of a built transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxSummary {
    pub label: String,
    pub signature: String,
    pub size: usize,
    pub account_count: usize,
    pub instruction_count: usize,
    pub blockhash: String,
    pub jito_tip_lamports: u64,
    pub priority_fee_micro_lamports: u64,
}

impl TxSummary {
    pub fn from_built(built: &BuiltTx, req: &TxRequest) -> Self {
        TxSummary {
            label: built.label.clone(),
            signature: built.signature().to_string(),
            size: built.size,
            account_count: built.account_count,
            instruction_count: built.instructions.len(),
            blockhash: built.blockhash.to_string(),
            jito_tip_lamports: req.jito_tip_lamports,
            priority_fee_micro_lamports: req.priority_fee_micro_lamports,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::compute_budget::ComputeBudgetInstruction;

    fn sample_ix() -> Instruction {
        Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(Pubkey::new_unique(), false)],
            data: vec![1, 2, 3],
        }
    }

    #[test]
    fn jito_tip_targets_a_known_account() {
        let payer = Pubkey::new_unique();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let ix = jito_tip_instruction(payer, 1_000_000);
            assert_eq!(ix.program_id, *SYSTEM_PROGRAM);
            // account 0 = payer, account 1 = tip destination
            let dest = ix.accounts[1].pubkey;
            assert!(
                JITO_TIP_ACCOUNTS.contains(&dest),
                "{dest} is not a Jito tip account"
            );
            seen.insert(dest);
        }
        assert!(
            seen.len() > 1,
            "tips should be spread across several accounts, got {seen:?}"
        );
    }

    #[test]
    fn tx_request_builder_is_chainable() {
        let req = TxRequest::new("snipe")
            .with_instruction(sample_ix())
            .priority_fee(1_000)
            .compute_units(200_000)
            .jito_tip(500_000)
            .wrap_sol(1_000_000);
        assert_eq!(req.label, "snipe");
        assert_eq!(req.instructions.len(), 1);
        assert_eq!(req.priority_fee_micro_lamports, 1_000);
        assert_eq!(req.compute_unit_limit, 200_000);
        assert_eq!(req.jito_tip_lamports, 500_000);
        assert_eq!(req.wrap_sol_lamports, 1_000_000);
    }

    #[test]
    fn manual_compile_puts_the_payer_first_and_dedupes() {
        let payer = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let shared = Pubkey::new_unique();
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(shared, false),
                AccountMeta::new_readonly(shared, false),
            ],
            data: vec![9],
        };
        let keys = metas(vec![
            (payer, true, true),
            (shared, true, false),
            (program, false, false),
        ]);
        let msg = compile_v0_manual(payer, Hash::default(), &[ix], keys, vec![]).unwrap();
        assert_eq!(msg.account_keys[0], payer, "payer must be index 0");
        assert_eq!(msg.header.num_required_signatures, 1);
        // `shared` appeared twice but must be stored once, with the union of
        // its permissions (writable).
        let count = msg.account_keys.iter().filter(|k| **k == shared).count();
        assert_eq!(count, 1, "duplicate accounts must be merged");
        assert_eq!(msg.instructions.len(), 1);
        assert_eq!(msg.instructions[0].data, vec![9]);
    }

    #[test]
    fn manual_compile_rejects_accounts_outside_the_key_set() {
        let payer = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let unknown = Pubkey::new_unique();
        let ix = Instruction {
            program_id: program,
            accounts: vec![AccountMeta::new(unknown, false)],
            data: vec![],
        };
        let keys = metas(vec![(payer, true, true), (program, false, false)]);
        let err = compile_v0_manual(payer, Hash::default(), &[ix], keys, vec![]).unwrap_err();
        assert!(err.to_string().contains("not in the static keys"), "{err}");
    }

    #[test]
    fn manual_compile_rejects_empty_instructions() {
        let payer = Pubkey::new_unique();
        assert!(compile_v0_manual(payer, Hash::default(), &[], vec![], vec![]).is_err());
    }

    #[test]
    fn metas_helper_maps_flags() {
        let pk = Pubkey::new_unique();
        let m = metas(vec![(pk, true, true), (pk, false, false)]);
        assert!(m[0].is_writable && m[0].is_signer);
        assert!(!m[1].is_writable && !m[1].is_signer);
    }

    #[test]
    fn fmt_sol_is_readable() {
        assert_eq!(fmt_sol(1_000_000_000), "1.000000 SOL");
        assert_eq!(fmt_sol(10_000_000), "0.010000 SOL");
    }

    #[test]
    fn budget_instructions_match_the_on_chain_tags() {
        // Tags verified against solana-sdk: SetComputeUnitLimit = 2,
        // SetComputeUnitPrice = 3.
        let limit = ComputeBudgetInstruction::set_compute_unit_limit(1);
        let price = ComputeBudgetInstruction::set_compute_unit_price(1);
        assert_eq!(limit.data[0], 2);
        assert_eq!(price.data[0], 3);
        assert_eq!(limit.program_id, *COMPUTE_BUDGET_PROGRAM);
    }

    // ------------------------------------------------------------------
    // Multi-signer transaction construction (signer abstraction)
    // ------------------------------------------------------------------

    use crate::signer::{LocalKeypairSigner, SignerRegistry, TransactionSigner};
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer as SdkSigner;
    use std::time::Duration;

    /// An RPC that is never called: every test pins `req.blockhash` and uses
    /// no wSOL wrapping, so the builder never touches the network.
    fn offline_rpc() -> Rpc {
        Rpc::with_urls(
            "http://127.0.0.1:1".to_string(),
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            0,
            Duration::from_millis(50),
        )
        .expect("rpc construction is offline")
    }

    fn transfer_from(from: &Pubkey, to: &Pubkey) -> Instruction {
        system_instruction::transfer(from, to, 1)
    }

    fn base_request(ixs: Vec<Instruction>) -> TxRequest {
        let mut req = TxRequest::new("multi-sig-test").with_instructions(ixs);
        req.blockhash = Some(Hash::default());
        req
    }

    fn message_bytes(tx: &VersionedTransaction) -> Vec<u8> {
        tx.message.serialize()
    }

    #[tokio::test]
    async fn single_signer_transaction_still_builds_and_verifies() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let req = base_request(vec![transfer_from(&wallet.pubkey, &Pubkey::new_unique())]);
        let built = TxBuilder::new(&rpc, &wallet)
            .build(&req)
            .await
            .expect("wallet-only build must keep working exactly as before");
        assert_eq!(built.tx.signatures.len(), 1);
        assert!(built.tx.signatures[0].verify(wallet.pubkey.as_ref(), &message_bytes(&built.tx)));
    }

    #[tokio::test]
    async fn primary_plus_one_extra_signer_builds_with_two_verifying_signatures() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let extra_kp = Keypair::new();
        let extra_pk = extra_kp.pubkey();

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "treasury",
                Arc::new(LocalKeypairSigner::new(
                    "treasury",
                    Wallet::from_keypair(extra_kp),
                )) as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        let mut req = base_request(vec![
            transfer_from(&wallet.pubkey, &Pubkey::new_unique()),
            transfer_from(&extra_pk, &Pubkey::new_unique()),
        ]);
        req.extra_signers = vec![extra_pk];

        let built = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect("two-signer build");
        assert_eq!(built.tx.signatures.len(), 2);
        let required = required_signer_keys(&built.tx.message);
        let bytes = message_bytes(&built.tx);
        for (i, key) in required.iter().enumerate() {
            assert!(
                built.tx.signatures[i].verify(key.as_ref(), &bytes),
                "signature {i} must verify for {key}"
            );
        }
        assert!(required.contains(&wallet.pubkey) && required.contains(&extra_pk));
    }

    #[tokio::test]
    async fn multiple_extra_signers_all_sign_in_message_order() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let kp_a = Keypair::new();
        let kp_b = Keypair::new();
        let (pk_a, pk_b) = (kp_a.pubkey(), kp_b.pubkey());

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "a",
                Arc::new(LocalKeypairSigner::new("a", Wallet::from_keypair(kp_a)))
                    as Arc<dyn TransactionSigner>,
            )
            .unwrap();
        registry
            .register(
                "b",
                Arc::new(LocalKeypairSigner::new("b", Wallet::from_keypair(kp_b)))
                    as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        let mut req = base_request(vec![
            transfer_from(&pk_b, &Pubkey::new_unique()),
            transfer_from(&pk_a, &Pubkey::new_unique()),
            transfer_from(&wallet.pubkey, &Pubkey::new_unique()),
        ]);
        req.extra_signers = vec![pk_a, pk_b];

        let built = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect("three-signer build");
        assert_eq!(built.tx.signatures.len(), 3);
        let required = required_signer_keys(&built.tx.message);
        let bytes = message_bytes(&built.tx);
        for (i, key) in required.iter().enumerate() {
            assert!(built.tx.signatures[i].verify(key.as_ref(), &bytes));
        }
    }

    #[tokio::test]
    async fn duplicate_extra_signer_entries_are_collapsed_and_signed_once() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let kp = Keypair::new();
        let pk = kp.pubkey();

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "dup",
                Arc::new(LocalKeypairSigner::new("dup", Wallet::from_keypair(kp)))
                    as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        let mut req = base_request(vec![transfer_from(&pk, &Pubkey::new_unique())]);
        req.extra_signers = vec![pk, pk, pk];

        let built = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect("duplicates must be handled safely");
        // wallet + the single deduplicated extra signer
        assert_eq!(built.tx.signatures.len(), 2);
        let bytes = message_bytes(&built.tx);
        for (i, key) in required_signer_keys(&built.tx.message).iter().enumerate() {
            assert!(built.tx.signatures[i].verify(key.as_ref(), &bytes));
        }
    }

    #[tokio::test]
    async fn missing_required_signer_fails_the_build() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let kp = Keypair::new();
        let pk = kp.pubkey();

        // Registry does NOT contain pk.
        let registry = SignerRegistry::new();
        let mut req = base_request(vec![transfer_from(&pk, &Pubkey::new_unique())]);
        req.extra_signers = vec![pk];

        let err = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect_err("unresolvable required signer must fail");
        match err {
            BotError::Signer(SignerError::MissingSigner { pubkey }) => {
                assert_eq!(pubkey, pk.to_string())
            }
            other => panic!("expected MissingSigner, got {other:?}"),
        }

        // Same request without any registry at all: identical hard failure.
        let err = TxBuilder::new(&rpc, &wallet)
            .build(&req)
            .await
            .expect_err("wallet-only builder must refuse extra signers");
        assert!(matches!(
            err,
            BotError::Signer(SignerError::MissingSigner { .. })
        ));
    }

    #[tokio::test]
    async fn extra_signer_not_required_by_message_is_rejected() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let kp = Keypair::new();
        let pk = kp.pubkey();

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "idle",
                Arc::new(LocalKeypairSigner::new("idle", Wallet::from_keypair(kp)))
                    as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        // The instructions only involve the wallet, but the caller declares
        // an extra signer: mismatch must be explicit, not silent.
        let mut req = base_request(vec![transfer_from(&wallet.pubkey, &Pubkey::new_unique())]);
        req.extra_signers = vec![pk];

        let err = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect_err("declared-but-unneeded signer must fail");
        assert!(matches!(
            err,
            BotError::Signer(SignerError::ExtraSignerNotRequired { .. })
        ));
    }

    #[tokio::test]
    async fn undeclared_required_signer_is_rejected_even_when_resolvable() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let kp = Keypair::new();
        let pk = kp.pubkey();

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "stealth",
                Arc::new(LocalKeypairSigner::new("stealth", Wallet::from_keypair(kp)))
                    as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        // Message requires pk, registry could sign for it, but the request
        // never declared it -> explicit signer-ownership violation.
        let req = base_request(vec![transfer_from(&pk, &Pubkey::new_unique())]);
        let err = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect_err("undeclared required signer must fail");
        match err {
            BotError::Signer(SignerError::SignerMismatch { expected, .. }) => {
                assert_eq!(expected, pk.to_string())
            }
            other => panic!("expected SignerMismatch, got {other:?}"),
        }
    }

    /// A signer whose backend fails (models a KMS/HSM outage).
    #[derive(Debug)]
    struct OutageSigner {
        pk: Pubkey,
    }

    #[async_trait::async_trait]
    impl TransactionSigner for OutageSigner {
        fn pubkey(&self) -> Pubkey {
            self.pk
        }
        async fn sign_message(&self, _message: &[u8]) -> Result<Signature, SignerError> {
            Err(SignerError::SigningFailed {
                context: "backend outage".into(),
            })
        }
    }

    #[tokio::test]
    async fn failed_signature_propagates_as_signing_failed() {
        let rpc = offline_rpc();
        let wallet = Wallet::generate();
        let pk = Pubkey::new_unique();

        let mut registry = SignerRegistry::new();
        registry
            .register(
                "outage",
                Arc::new(OutageSigner { pk }) as Arc<dyn TransactionSigner>,
            )
            .unwrap();

        let mut req = base_request(vec![transfer_from(&pk, &Pubkey::new_unique())]);
        req.extra_signers = vec![pk];

        let err = TxBuilder::with_registry(&rpc, &wallet, Arc::new(registry))
            .build(&req)
            .await
            .expect_err("backend failure must propagate");
        match err {
            BotError::Signer(SignerError::SigningFailed { context }) => {
                assert!(context.contains("backend outage"), "{context}");
                assert!(context.contains("multi-sig-test"), "label aids triage");
            }
            other => panic!("expected SigningFailed, got {other:?}"),
        }
    }

    #[test]
    fn required_signer_keys_matches_header_count_and_order() {
        let payer = Keypair::new();
        let co = Keypair::new();
        let msg = MessageV0::try_compile(
            &payer.pubkey(),
            &[
                system_instruction::transfer(&payer.pubkey(), &Pubkey::new_unique(), 1),
                system_instruction::transfer(&co.pubkey(), &Pubkey::new_unique(), 1),
            ],
            &[],
            Hash::default(),
        )
        .unwrap();
        let required = required_signer_keys(&VersionedMessage::V0(msg.clone()));
        assert_eq!(required.len(), msg.header.num_required_signatures as usize);
        assert_eq!(required[0], payer.pubkey(), "fee payer signs first");
        assert!(required.contains(&co.pubkey()));
    }
}
