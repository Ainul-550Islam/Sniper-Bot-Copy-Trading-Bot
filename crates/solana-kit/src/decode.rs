//! Turn a confirmed transaction back into a structured swap.
//!
//! Module 2 (copy trading) is built on this: watch a wallet's signatures, fetch
//! each transaction, and answer "what did they buy, how much did they spend,
//! and on which venue?" The answer comes from the token-balance deltas the RPC
//! already computed (`preTokenBalances` / `postTokenBalances`) rather than from
//! instruction data, which is venue-specific and changes without notice.
//!
//! Balance deltas are also the only approach that survives a Jupiter route:
//! the outer instruction is a Jupiter swap, but the deltas show the SOL going
//! out and the token coming in regardless of how many hops it took.

use std::collections::HashMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{EncodedTransaction, UiTransactionTokenBalance};
use tracing::debug;

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::events::{self, PumpEvent};

/// Which program executed the swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapVenue {
    /// Still on the bonding curve.
    PumpBondingCurve,
    /// Graduated to the pump AMM.
    PumpSwap,
    RaydiumAmmV4,
    RaydiumClmm,
    RaydiumCpmm,
    Jupiter,
    /// A DEX we do not model. The deltas are still valid.
    Other,
    /// No recognisable swap program: a plain transfer, an ATA creation, …
    None,
}

impl SwapVenue {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwapVenue::PumpBondingCurve => "pump.fun",
            SwapVenue::PumpSwap => "pumpswap",
            SwapVenue::RaydiumAmmV4 => "raydium_v4",
            SwapVenue::RaydiumClmm => "raydium_clmm",
            SwapVenue::RaydiumCpmm => "raydium_cpmm",
            SwapVenue::Jupiter => "jupiter",
            SwapVenue::Other => "other",
            SwapVenue::None => "none",
        }
    }

    /// Venues worth copying. A plain transfer is not a trade.
    pub fn is_dex(&self) -> bool {
        !matches!(self, SwapVenue::None | SwapVenue::Other)
    }
}

/// A decoded swap performed by one wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedSwap {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub wallet: String,
    pub side: Side,
    pub venue: SwapVenue,
    /// The non-quote token.
    pub base_mint: String,
    /// The quote token (WSOL for a SOL pair, USDC for a stable pair).
    pub quote_mint: String,
    /// Raw units, in the base mint's decimals.
    pub base_amount: u64,
    /// Raw units, in the quote mint's decimals.
    pub quote_amount: u64,
    pub base_decimals: u8,
    pub quote_decimals: u8,
    /// Fee paid in lamports.
    pub fee_lamports: u64,
    /// `base_amount / quote_amount`, normalised to human decimals.
    pub price_quote_per_base: f64,
    /// Every program id the transaction executed, outermost first.
    pub programs: Vec<String>,
    /// Pump events emitted by the transaction, when any.
    pub events: Vec<PumpEvent>,
    /// The log lines, kept so a failed decode can be diagnosed.
    pub logs: Vec<String>,
    pub succeeded: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(&self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

impl DecodedSwap {
    /// Base amount in human units.
    pub fn base_human(&self) -> f64 {
        maths::from_raw_amount(self.base_amount, self.base_decimals)
    }

    /// Quote amount in human units.
    pub fn quote_human(&self) -> f64 {
        maths::from_raw_amount(self.quote_amount, self.quote_decimals)
    }

    /// Quote amount in SOL, when the quote mint is WSOL.
    pub fn sol_value(&self) -> Option<f64> {
        if self.quote_mint == WSOL_MINT.to_string() {
            Some(maths::lamports_to_sol(self.quote_amount))
        } else {
            None
        }
    }

    /// One line for Telegram.
    pub fn describe(&self) -> String {
        let side = match self.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        let value = self
            .sol_value()
            .map(|s| format!(" for {s:.4} SOL"))
            .unwrap_or_else(|| format!(" for {:.4} quote", self.quote_human()));
        format!(
            "{side} {:.2} of {}{value} on {} ({})",
            self.base_human(),
            self.base_mint,
            self.venue.as_str(),
            self.signature.chars().take(8).collect::<String>(),
        )
    }

    /// Minimum SOL value worth copying, for the Module 2 whale filter.
    pub fn meets_min_sol(&self, min_sol: f64) -> bool {
        match self.sol_value() {
            Some(v) => v >= min_sol,
            // A non-SOL pair: fall back to the raw quote amount being non-trivial.
            None => self.quote_amount > 0,
        }
    }
}

/// Per-account balance movement for one mint.
#[derive(Debug, Clone, Default)]
struct BalanceDelta {
    mint: Pubkey,
    decimals: u8,
    owner: Option<Pubkey>,
    pre: u64,
    post: u64,
    account_index: u8,
}

impl BalanceDelta {
    /// Signed change; positive means the account received tokens.
    fn change(&self) -> i128 {
        self.post as i128 - self.pre as i128
    }
}

/// Decode a transaction fetched via `getTransaction` into a swap for `wallet`.
///
/// Returns `Ok(None)` when the transaction contains no swap involving that
/// wallet — that is the common case and not an error.
pub fn decode_swap(
    wallet: &Pubkey,
    signature: &str,
    slot: u64,
    block_time: Option<i64>,
    tx: &EncodedTransaction,
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> BotResult<Option<DecodedSwap>> {
    let wallet_str = wallet.to_string();

    // ---- account keys ----------------------------------------------------
    // For a v0 transaction the static keys are in the message and the lookup
    // table addresses are in `meta.loaded_addresses`; the balance arrays index
    // into the concatenation.
    let account_keys = account_keys(tx, meta)?;

    // ---- programs executed ----------------------------------------------
    let programs = executed_programs(tx, &account_keys);
    let venue = classify_venue(&programs, meta);

    // ---- token balance deltas -------------------------------------------
    let pre = token_balances(&meta.pre_token_balances);
    let post = token_balances(&meta.post_token_balances);
    let deltas = merge_deltas(&pre, &post, &account_keys);

    // Only consider accounts this wallet owns. The RPC fills in `owner` for
    // every balance entry, so an ATA belonging to somebody else in the same
    // transaction (a pool vault, say) is excluded.
    let mine: Vec<&BalanceDelta> = deltas
        .iter()
        .filter(|d| {
            d.owner.map(|o| o == *wallet).unwrap_or(false)
                || account_index_owner(&account_keys, d.account_index)
                    .map(|o| o == *wallet)
                    .unwrap_or(false)
        })
        .filter(|d| d.change() != 0)
        .collect();

    if mine.is_empty() {
        debug!(%signature, "no token balance movement for the tracked wallet");
        return Ok(None);
    }

    // ---- split into quote-side and base-side -----------------------------
    // The quote side is WSOL, or a stablecoin if the pair is quoted in one.
    let (quote, base) = match split_quote_base(&mine) {
        Some(pair) => pair,
        None => {
            debug!(
                %signature,
                movements = mine.len(),
                "no quote/base pair — a transfer or a single-sided movement, not a swap"
            );
            return Ok(None);
        }
    };

    let quote_out = quote.change() < 0;
    let side = if quote_out { Side::Buy } else { Side::Sell };

    let logs = match &meta.log_messages {
        OptionSerializer::Some(l) => l.clone(),
        _ => Vec::new(),
    };
    let parsed_events = events::parse_logs(&logs);

    // Token balances are u64 on chain; the i128 delta only exists so the
    // sign survives the subtraction. Clamp on the way back down.
    let base_amount = base.change().unsigned_abs().min(u64::MAX as u128) as u64;
    let quote_amount = quote.change().unsigned_abs().min(u64::MAX as u128) as u64;
    let price = if base_amount == 0 {
        0.0
    } else {
        maths::from_raw_amount(quote_amount, quote.decimals)
            / maths::from_raw_amount(base_amount, base.decimals)
    };

    Ok(Some(DecodedSwap {
        signature: signature.to_string(),
        slot,
        block_time,
        wallet: wallet_str,
        side,
        venue,
        base_mint: base.mint.to_string(),
        quote_mint: quote.mint.to_string(),
        base_amount,
        quote_amount,
        base_decimals: base.decimals,
        quote_decimals: quote.decimals,
        fee_lamports: meta.fee,
        price_quote_per_base: price,
        programs: programs.iter().map(|p| p.to_string()).collect(),
        events: parsed_events,
        logs,
        succeeded: meta.err.is_none(),
        error: meta.err.as_ref().map(|e| e.to_string()),
    }))
}

/// Classify the venue from the programs that ran, including inner instructions.
fn classify_venue(
    programs: &[Pubkey],
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> SwapVenue {
    let mut all: Vec<Pubkey> = programs.to_vec();
    if let OptionSerializer::Some(inner) = &meta.inner_instructions {
        for group in inner {
            for ix in &group.instructions {
                // `UiInstruction::Compiled` carries a program_id_index; the
                // parsed forms do not, and are not requested.
                if let solana_transaction_status::UiInstruction::Compiled(c) = ix {
                    if let Some(pk) = all.get(c.program_id_index as usize) {
                        all.push(*pk);
                    }
                }
            }
        }
    }

    // Order matters: Jupiter routes *through* the other venues, so a Jupiter
    // program id anywhere in the transaction wins.
    if all.contains(&*JUPITER_V6_PROGRAM) {
        return SwapVenue::Jupiter;
    }
    if all.contains(&*PUMP_PROGRAM_ID) {
        return SwapVenue::PumpBondingCurve;
    }
    if all.contains(&*PUMPSWAP_PROGRAM_ID) {
        return SwapVenue::PumpSwap;
    }
    if all.contains(&*RAYDIUM_AMM_V4) {
        return SwapVenue::RaydiumAmmV4;
    }
    if all.contains(&*RAYDIUM_CLMM) {
        return SwapVenue::RaydiumClmm;
    }
    if all.contains(&*RAYDIUM_CPMM) {
        return SwapVenue::RaydiumCpmm;
    }
    // Any unrecognised program beyond the usual system/token noise.
    let noise = [
        *SYSTEM_PROGRAM,
        *TOKEN_PROGRAM,
        *TOKEN_2022_PROGRAM,
        *ASSOCIATED_TOKEN_PROGRAM,
        *COMPUTE_BUDGET_PROGRAM,
        *RENT_SYSVAR,
    ];
    if all.iter().any(|p| !noise.contains(p)) {
        return SwapVenue::Other;
    }
    SwapVenue::None
}

/// Pull the executable program ids out of the message, in order.
///
/// `EncodedTransaction::decode()` only handles the binary encodings, so the
/// `json` form is walked by hand: its `programIdIndex` values index into the
/// same account-key list we already reconstructed.
fn executed_programs(tx: &EncodedTransaction, account_keys: &[Pubkey]) -> Vec<Pubkey> {
    let id_indices: Vec<u8> = match tx {
        EncodedTransaction::Json(json) => match &json.message {
            solana_transaction_status::UiMessage::Raw(raw) => raw
                .instructions
                .iter()
                .map(|ix| ix.program_id_index)
                .collect(),
            solana_transaction_status::UiMessage::Parsed(_) => return Vec::new(),
        },
        other => {
            let Some(decoded) = decode_transaction(other) else {
                return Vec::new();
            };
            decoded
                .message
                .instructions()
                .iter()
                .map(|ix| ix.program_id_index)
                .collect()
        }
    };
    id_indices
        .iter()
        .filter_map(|&i| account_keys.get(i as usize).copied())
        .collect()
}

/// Reconstruct the full account-key list, static keys followed by the writable
/// then readonly addresses loaded from lookup tables.
fn account_keys(
    tx: &EncodedTransaction,
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> BotResult<Vec<Pubkey>> {
    let mut keys: Vec<Pubkey> = match tx {
        // `json`/`jsonParsed` encoding: the keys are already strings.
        EncodedTransaction::Json(json) => match &json.message {
            solana_transaction_status::UiMessage::Raw(raw) => raw
                .account_keys
                .iter()
                .filter_map(|s| Pubkey::from_str(s).ok())
                .collect(),
            solana_transaction_status::UiMessage::Parsed(_) => {
                return Err(BotError::encoding(
                    "jsonParsed-encoded transactions carry no raw account keys; request `json` or `base64` encoding",
                ))
            }
        },
        EncodedTransaction::Accounts(_) => {
            return Err(BotError::encoding(
                "accounts-encoded transactions are not supported; request base64 or json",
            ))
        }
        // base64 / base58: decode the message itself.
        other => {
            let decoded = decode_transaction(other).ok_or_else(|| {
                BotError::encoding(
                    "could not deserialize the transaction; request base64 encoding",
                )
            })?;
            decoded.message.static_account_keys().to_vec()
        }
    };

    if let OptionSerializer::Some(loaded) = &meta.loaded_addresses {
        for s in loaded.writable.iter().chain(loaded.readonly.iter()) {
            if let Ok(pk) = Pubkey::from_str(s) {
                keys.push(pk);
            }
        }
    }
    Ok(keys)
}

/// Deserialize the transaction regardless of which encoding the RPC used.
fn decode_transaction(
    tx: &EncodedTransaction,
) -> Option<solana_sdk::transaction::VersionedTransaction> {
    tx.decode()
}

/// One top-level instruction of a confirmed transaction with its account
/// indices resolved to pubkeys. Used by protocol-specific detectors (Raydium
/// pool initialisation is not an Anchor event, so it has to be read from the
/// instruction itself rather than from a `Program data:` log line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedInstruction {
    pub program_id: Pubkey,
    pub accounts: Vec<Pubkey>,
    pub data: Vec<u8>,
}

/// Resolve every top-level instruction of `tx` (program id, account pubkeys,
/// raw data). Lookup-table addresses come from `meta.loaded_addresses`, so a
/// v0 transaction resolves exactly like a legacy one. Inner (CPI)
/// instructions are not included: the notification meta carries them only
/// as indices into the same key list and no current detector needs them.
pub fn decode_instructions(
    tx: &EncodedTransaction,
    meta: &solana_transaction_status::UiTransactionStatusMeta,
) -> BotResult<Vec<DecodedInstruction>> {
    let keys = account_keys(tx, meta)?;
    let resolve =
        |program_id_index: u8, accounts: &[u8], data: Vec<u8>| -> Option<DecodedInstruction> {
            let program_id = *keys.get(program_id_index as usize)?;
            let accounts = accounts
                .iter()
                .map(|i| keys.get(*i as usize).copied())
                .collect::<Option<Vec<Pubkey>>>()?;
            Some(DecodedInstruction {
                program_id,
                accounts,
                data,
            })
        };

    let out = match tx {
        EncodedTransaction::Json(json) => match &json.message {
            solana_transaction_status::UiMessage::Raw(raw) => raw
                .instructions
                .iter()
                .filter_map(|ix| {
                    let data = bs58::decode(&ix.data).into_vec().ok()?;
                    resolve(ix.program_id_index, &ix.accounts, data)
                })
                .collect(),
            solana_transaction_status::UiMessage::Parsed(_) => {
                return Err(BotError::encoding(
                    "jsonParsed-encoded transactions carry no raw instructions; request `json` or `base64` encoding",
                ))
            }
        },
        other => {
            let decoded = decode_transaction(other).ok_or_else(|| {
                BotError::encoding(
                    "could not deserialize the transaction; request base64 encoding",
                )
            })?;
            decoded
                .message
                .instructions()
                .iter()
                .filter_map(|ix| resolve(ix.program_id_index, &ix.accounts, ix.data.clone()))
                .collect()
        }
    };
    Ok(out)
}

/// Index the balance entries by `account_index`.
fn token_balances(
    entries: &OptionSerializer<Vec<UiTransactionTokenBalance>>,
) -> HashMap<u8, &UiTransactionTokenBalance> {
    let mut map = HashMap::new();
    if let OptionSerializer::Some(list) = entries {
        for b in list {
            map.insert(b.account_index, b);
        }
    }
    map
}

/// Merge pre/post entries into per-account deltas.
fn merge_deltas<'a>(
    pre: &HashMap<u8, &'a UiTransactionTokenBalance>,
    post: &HashMap<u8, &'a UiTransactionTokenBalance>,
    _account_keys: &[Pubkey],
) -> Vec<BalanceDelta> {
    let mut indices: Vec<u8> = pre.keys().copied().collect();
    for k in post.keys() {
        if !indices.contains(k) {
            indices.push(*k);
        }
    }
    indices.sort_unstable();

    let amount = |b: Option<&&UiTransactionTokenBalance>| -> u64 {
        b.and_then(|b| b.ui_token_amount.amount.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let decimals = |b: Option<&&UiTransactionTokenBalance>| -> u8 {
        b.map(|b| b.ui_token_amount.decimals).unwrap_or(0)
    };
    let owner = |b: Option<&&UiTransactionTokenBalance>| -> Option<Pubkey> {
        b.and_then(|b| match &b.owner {
            OptionSerializer::Some(o) => Pubkey::from_str(o).ok(),
            _ => None,
        })
    };

    indices
        .into_iter()
        .map(|i| {
            let p = pre.get(&i);
            let q = post.get(&i);
            let mint_str = p.or(q).map(|b| b.mint.as_str()).unwrap_or_default();
            BalanceDelta {
                mint: Pubkey::from_str(mint_str).unwrap_or_default(),
                decimals: decimals(q).max(decimals(p)),
                owner: owner(q).or_else(|| owner(p)),
                pre: amount(p),
                post: amount(q),
                account_index: i,
            }
        })
        .collect()
}

/// The owner of an account key, when the key list says so. Used as a fallback
/// when the RPC omits `owner` from a balance entry.
fn account_index_owner(account_keys: &[Pubkey], index: u8) -> Option<Pubkey> {
    account_keys.get(index as usize).copied()
}

/// Split the movements into the quote leg and the base leg.
///
/// The quote leg is the one whose mint is WSOL or a known stablecoin. If there
/// are two candidates (a stable-to-stable move) or none, we cannot infer a
/// direction and return `None`.
fn split_quote_base<'a>(
    deltas: &[&'a BalanceDelta],
) -> Option<(&'a BalanceDelta, &'a BalanceDelta)> {
    if deltas.len() < 2 {
        return None;
    }
    let is_quote = |m: &Pubkey| *m == *WSOL_MINT || *m == *USDC_MINT;

    let quotes: Vec<&&BalanceDelta> = deltas.iter().filter(|d| is_quote(&d.mint)).collect();
    if quotes.len() != 1 {
        return None;
    }
    let quote = *quotes[0];

    // The base leg is the largest non-quote movement by absolute change. With
    // more than one, prefer the one that moved in the opposite direction — that
    // is what a swap looks like.
    let mut candidates: Vec<&&BalanceDelta> = deltas
        .iter()
        .filter(|d| !is_quote(&d.mint))
        .filter(|d| (d.change() > 0) != (quote.change() > 0))
        .collect();
    if candidates.is_empty() {
        candidates = deltas.iter().filter(|d| !is_quote(&d.mint)).collect();
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|d| std::cmp::Reverse(d.change().unsigned_abs()));
    Some((quote, *candidates[0]))
}

/// A parsed `transactionSubscribe` notification (Geyser/Yellowstone-style
/// websocket, BUILD PLAN §5). Providers deliver the same payload shape as
/// `getTransaction`: an encoded transaction plus its status meta — exactly
/// what [`decode_swap`] consumes — so the copy-trade pipeline is identical
/// for polled and pushed transactions.
#[derive(Debug, Clone)]
pub struct TxNotification {
    pub signature: String,
    pub slot: u64,
    /// Providers that include it send `blockTime`; others omit it.
    pub block_time: Option<i64>,
    pub transaction: EncodedTransaction,
    pub meta: solana_transaction_status::UiTransactionStatusMeta,
}

impl TxNotification {
    /// False when the transaction failed on chain (its token deltas are then
    /// partial or meaningless for copy purposes).
    pub fn succeeded(&self) -> bool {
        self.meta.err.is_none()
    }

    /// The program log lines from the meta, or an empty slice when the
    /// provider omitted them. Feed the result to
    /// [`crate::events::find_launch`] to spot token creations.
    pub fn log_messages(&self) -> &[String] {
        match &self.meta.log_messages {
            OptionSerializer::Some(logs) => logs,
            _ => &[],
        }
    }
}

/// Parse the `result` value of a `transactionNotification`. Returns `None`
/// when the payload is missing a field we cannot work without (signature,
/// transaction, meta) or when either fails to deserialize — a provider
/// schema drift must degrade the feed, not crash it.
pub fn parse_transaction_notification(raw: &serde_json::Value) -> Option<TxNotification> {
    let signature = raw.get("signature")?.as_str()?.to_string();
    if signature.is_empty() {
        return None;
    }
    let slot = raw.get("slot").and_then(|s| s.as_u64()).unwrap_or(0);
    let block_time = raw.get("blockTime").and_then(|t| t.as_i64());
    let transaction: EncodedTransaction =
        serde_json::from_value(raw.get("transaction")?.clone()).ok()?;
    let meta = serde_json::from_value(raw.get("meta")?.clone()).ok()?;
    Some(TxNotification {
        signature,
        slot,
        block_time,
        transaction,
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use solana_sdk::hash::Hash;
    use solana_sdk::message::v0::Message as MessageV0;
    use solana_sdk::message::MessageHeader;
    use solana_sdk::message::VersionedMessage;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    use solana_transaction_status::UiTransactionStatusMeta;
    use solana_transaction_status::{
        UiAccountsList, UiCompiledInstruction, UiMessage, UiParsedMessage, UiRawMessage,
        UiTransaction,
    };

    /// Build a base64-encoded v0 transaction that mentions `programs`.
    fn encoded_tx(payer: &Pubkey, programs: &[Pubkey], keypair: &Keypair) -> EncodedTransaction {
        let ixs: Vec<solana_sdk::instruction::Instruction> = programs
            .iter()
            .map(|p| solana_sdk::instruction::Instruction {
                program_id: *p,
                accounts: vec![solana_sdk::instruction::AccountMeta::new(*payer, true)],
                data: vec![1],
            })
            .collect();
        let msg = MessageV0::try_compile(payer, &ixs, &[], Hash::default()).unwrap();
        let tx = solana_sdk::transaction::VersionedTransaction::try_new(
            VersionedMessage::V0(msg),
            &[keypair],
        )
        .unwrap();
        let bytes = bincode::serialize(&tx).unwrap();
        EncodedTransaction::Binary(
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            solana_transaction_status::TransactionBinaryEncoding::Base64,
        )
    }

    fn balance(
        index: u8,
        mint: Pubkey,
        amount: u64,
        decimals: u8,
        owner: Pubkey,
    ) -> UiTransactionTokenBalance {
        // Built through serde so the test never has to name `UiTokenAmount`,
        // which lives in a transitive client-types crate.
        serde_json::from_value(serde_json::json!({
            "accountIndex": index,
            "mint": mint.to_string(),
            "uiTokenAmount": {
                "uiAmount": null,
                "decimals": decimals,
                "amount": amount.to_string(),
                "uiAmountString": ""
            },
            "owner": owner.to_string()
        }))
        .unwrap()
    }

    fn base_meta() -> UiTransactionStatusMeta {
        UiTransactionStatusMeta {
            err: None,
            status: Ok(()),
            fee: 5000,
            pre_balances: vec![],
            post_balances: vec![],
            inner_instructions: OptionSerializer::Skip,
            log_messages: OptionSerializer::Skip,
            pre_token_balances: OptionSerializer::Skip,
            post_token_balances: OptionSerializer::Skip,
            rewards: OptionSerializer::Skip,
            loaded_addresses: OptionSerializer::Skip,
            return_data: OptionSerializer::Skip,
            compute_units_consumed: OptionSerializer::Skip,
            cost_units: OptionSerializer::Skip,
        }
    }

    fn meta_with(
        pre: Vec<UiTransactionTokenBalance>,
        post: Vec<UiTransactionTokenBalance>,
        _programs: &[Pubkey],
    ) -> UiTransactionStatusMeta {
        UiTransactionStatusMeta {
            pre_token_balances: OptionSerializer::Some(pre),
            post_token_balances: OptionSerializer::Some(post),
            ..base_meta()
        }
    }

    #[test]
    fn decodes_a_buy_from_the_balance_deltas() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        // account_index 1 = wallet's wSOL ATA, 2 = wallet's token ATA.
        let pre = vec![
            balance(1, *WSOL_MINT, 5_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 4_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 12_345_678, 6, wallet.pubkey()),
        ];
        let tx = encoded_tx(
            &wallet.pubkey(),
            &[*PUMP_PROGRAM_ID, *TOKEN_PROGRAM],
            &wallet,
        );
        let meta = meta_with(pre, post, &[]);

        let swap = decode_swap(
            &wallet.pubkey(),
            "sig1",
            100,
            Some(1_750_000_000),
            &tx,
            &meta,
        )
        .unwrap()
        .expect("a swap must be decoded");

        assert_eq!(swap.side, Side::Buy);
        assert_eq!(swap.venue, SwapVenue::PumpBondingCurve);
        assert_eq!(swap.base_mint, token.to_string());
        assert_eq!(swap.quote_mint, WSOL_MINT.to_string());
        assert_eq!(swap.base_amount, 12_345_678);
        assert_eq!(swap.quote_amount, 1_000_000_000);
        assert_eq!(swap.base_decimals, 6);
        assert_eq!(swap.quote_decimals, 9);
        assert_eq!(swap.fee_lamports, 5000);
        assert!((swap.sol_value().unwrap() - 1.0).abs() < 1e-9);
        assert!(swap.succeeded);
        assert!(swap.programs.contains(&PUMP_PROGRAM_ID.to_string()));
        // 1 SOL for 12.345678 tokens => 0.081 SOL per token.
        assert!((swap.price_quote_per_base - 1.0 / 12.345678).abs() < 1e-6);
    }

    #[test]
    fn decodes_a_sell_as_the_opposite_direction() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        let pre = vec![
            balance(1, *WSOL_MINT, 1_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 5_000_000, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 1_900_000_000, 9, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let tx = encoded_tx(&wallet.pubkey(), &[*PUMPSWAP_PROGRAM_ID], &wallet);
        let swap = decode_swap(
            &wallet.pubkey(),
            "sig2",
            101,
            None,
            &tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(swap.side, Side::Sell);
        assert_eq!(swap.venue, SwapVenue::PumpSwap);
        assert_eq!(swap.base_amount, 5_000_000);
        assert_eq!(swap.quote_amount, 900_000_000);
        assert_eq!(swap.side.opposite(), Side::Buy);
    }

    #[test]
    fn ignores_movements_in_accounts_the_wallet_does_not_own() {
        let wallet = Keypair::new();
        let other = Pubkey::new_unique();
        let token = Pubkey::new_unique();
        // The pool's vaults move too; they must not be attributed to us.
        let pre = vec![
            balance(1, *WSOL_MINT, 900_000_000_000, 9, other),
            balance(2, token, 1_000_000_000, 6, other),
            balance(3, *WSOL_MINT, 2_000_000_000, 9, wallet.pubkey()),
            balance(4, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 901_000_000_000, 9, other),
            balance(2, token, 990_000_000, 6, other),
            balance(3, *WSOL_MINT, 1_000_000_000, 9, wallet.pubkey()),
            balance(4, token, 10_000_000, 6, wallet.pubkey()),
        ];
        let tx = encoded_tx(&wallet.pubkey(), &[*RAYDIUM_AMM_V4], &wallet);
        let swap = decode_swap(
            &wallet.pubkey(),
            "sig3",
            102,
            None,
            &tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(swap.quote_amount, 1_000_000_000, "only our own leg");
        assert_eq!(swap.base_amount, 10_000_000);
        assert_eq!(swap.venue, SwapVenue::RaydiumAmmV4);
    }

    #[test]
    fn a_plain_transfer_is_not_a_swap() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        // Only the token moved; no quote leg.
        let pre = vec![balance(2, token, 5_000_000, 6, wallet.pubkey())];
        let post = vec![balance(2, token, 1_000_000, 6, wallet.pubkey())];
        let tx = encoded_tx(&wallet.pubkey(), &[*TOKEN_PROGRAM], &wallet);
        let out = decode_swap(
            &wallet.pubkey(),
            "sig4",
            103,
            None,
            &tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap();
        assert!(out.is_none(), "a single-sided movement is not a swap");
    }

    #[test]
    fn no_balance_movement_returns_none() {
        let wallet = Keypair::new();
        let tx = encoded_tx(&wallet.pubkey(), &[*SYSTEM_PROGRAM], &wallet);
        let out = decode_swap(
            &wallet.pubkey(),
            "sig5",
            104,
            None,
            &tx,
            &meta_with(vec![], vec![], &[]),
        )
        .unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn venue_classification_prefers_jupiter() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        let pre = vec![
            balance(1, *WSOL_MINT, 2_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 1_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 1_000_000, 6, wallet.pubkey()),
        ];
        // A Jupiter route that hops through Raydium.
        let tx = encoded_tx(
            &wallet.pubkey(),
            &[*JUPITER_V6_PROGRAM, *RAYDIUM_AMM_V4, *TOKEN_PROGRAM],
            &wallet,
        );
        let swap = decode_swap(
            &wallet.pubkey(),
            "sig6",
            105,
            None,
            &tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(swap.venue, SwapVenue::Jupiter);
    }

    #[test]
    fn venue_classification_handles_noise_only() {
        let meta = base_meta();
        assert_eq!(
            classify_venue(&[*SYSTEM_PROGRAM, *TOKEN_PROGRAM], &meta),
            SwapVenue::None
        );
        assert_eq!(
            classify_venue(&[*RAYDIUM_CLMM], &meta),
            SwapVenue::RaydiumClmm
        );
        assert_eq!(
            classify_venue(&[*RAYDIUM_CPMM], &meta),
            SwapVenue::RaydiumCpmm
        );
        let random = Pubkey::new_unique();
        assert_eq!(classify_venue(&[random], &meta), SwapVenue::Other);
        assert!(!SwapVenue::Other.is_dex());
        assert!(SwapVenue::Jupiter.is_dex());
    }

    #[test]
    fn usdc_quoted_pairs_are_recognised() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        let pre = vec![
            balance(1, *USDC_MINT, 1_000_000_000, 6, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *USDC_MINT, 500_000_000, 6, wallet.pubkey()),
            balance(2, token, 2_000_000, 6, wallet.pubkey()),
        ];
        let tx = encoded_tx(&wallet.pubkey(), &[*JUPITER_V6_PROGRAM], &wallet);
        let swap = decode_swap(
            &wallet.pubkey(),
            "sig7",
            106,
            None,
            &tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(swap.quote_mint, USDC_MINT.to_string());
        assert_eq!(swap.side, Side::Buy);
        assert!(swap.sol_value().is_none(), "a USDC quote has no SOL value");
        assert!(swap.meets_min_sol(0.0));
    }

    #[test]
    fn failed_transactions_are_flagged_but_still_decoded() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        let pre = vec![
            balance(1, *WSOL_MINT, 2_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 1_500_000_000, 9, wallet.pubkey()),
            balance(2, token, 100, 6, wallet.pubkey()),
        ];
        let tx = encoded_tx(&wallet.pubkey(), &[*PUMP_PROGRAM_ID], &wallet);
        let mut meta = meta_with(pre, post, &[]);
        meta.err = Some(solana_sdk::transaction::TransactionError::InstructionError(
            0,
            solana_sdk::instruction::InstructionError::Custom(6024),
        ));
        let swap = decode_swap(&wallet.pubkey(), "sig8", 107, None, &tx, &meta)
            .unwrap()
            .unwrap();
        assert!(!swap.succeeded);
        assert!(
            swap.error
                .as_deref()
                .unwrap()
                .contains("custom program error"),
            "{}",
            swap.error.as_deref().unwrap_or_default()
        );
    }

    #[test]
    fn loaded_addresses_extend_the_account_key_list() {
        let wallet = Keypair::new();
        let tx = encoded_tx(&wallet.pubkey(), &[*PUMP_PROGRAM_ID], &wallet);
        let mut meta = meta_with(vec![], vec![], &[]);
        let extra = Pubkey::new_unique();
        meta.loaded_addresses =
            OptionSerializer::Some(solana_transaction_status::UiLoadedAddresses {
                writable: vec![extra.to_string()],
                readonly: vec![],
            });
        let keys = account_keys(&tx, &meta).unwrap();
        assert_eq!(*keys.last().unwrap(), extra);
    }

    #[test]
    fn describe_is_human_readable() {
        let d = DecodedSwap {
            signature: "5xyZabcdef1234567890".into(),
            slot: 1,
            block_time: None,
            wallet: Pubkey::new_unique().to_string(),
            side: Side::Buy,
            venue: SwapVenue::PumpBondingCurve,
            base_mint: Pubkey::new_unique().to_string(),
            quote_mint: WSOL_MINT.to_string(),
            base_amount: 12_500_000,
            quote_amount: 1_000_000_000,
            base_decimals: 6,
            quote_decimals: 9,
            fee_lamports: 5000,
            price_quote_per_base: 0.08,
            programs: vec![],
            events: vec![],
            logs: vec![],
            succeeded: true,
            error: None,
        };
        let s = d.describe();
        assert!(s.starts_with("BUY 12.50 of"), "{s}");
        assert!(s.contains("1.0000 SOL"), "{s}");
        assert!(s.contains("pump.fun"), "{s}");
        assert!(d.meets_min_sol(0.5));
        assert!(!d.meets_min_sol(2.0));
    }

    #[test]
    fn merge_deltas_handles_accounts_present_on_only_one_side() {
        let wallet = Pubkey::new_unique();
        let token = Pubkey::new_unique();
        // The ATA did not exist before: only a post entry.
        let pre = vec![balance(1, *WSOL_MINT, 2_000_000_000, 9, wallet)];
        let post = vec![
            balance(1, *WSOL_MINT, 1_000_000_000, 9, wallet),
            balance(2, token, 500, 6, wallet),
        ];
        let pre_opt = OptionSerializer::Some(pre);
        let post_opt = OptionSerializer::Some(post);
        let pm = token_balances(&pre_opt);
        let qm = token_balances(&post_opt);
        let deltas = merge_deltas(&pm, &qm, &[]);
        assert_eq!(deltas.len(), 2);
        let token_delta = deltas.iter().find(|d| d.mint == token).unwrap();
        assert_eq!(token_delta.pre, 0);
        assert_eq!(token_delta.post, 500);
        assert_eq!(token_delta.change(), 500);
        assert_eq!(token_delta.decimals, 6);
    }

    #[test]
    fn split_rejects_two_quote_legs() {
        let wallet = Pubkey::new_unique();
        let d1 = BalanceDelta {
            mint: *WSOL_MINT,
            decimals: 9,
            owner: Some(wallet),
            pre: 10,
            post: 5,
            account_index: 0,
        };
        let d2 = BalanceDelta {
            mint: *USDC_MINT,
            decimals: 6,
            owner: Some(wallet),
            pre: 0,
            post: 100,
            account_index: 1,
        };
        assert!(
            split_quote_base(&[&d1, &d2]).is_none(),
            "two quote legs give no unambiguous base"
        );
    }

    #[test]
    fn binary_message_decoding_round_trips() {
        let wallet = Keypair::new();
        let tx = encoded_tx(&wallet.pubkey(), &[*PUMP_PROGRAM_ID], &wallet);
        let meta = meta_with(vec![], vec![], &[]);
        let keys = account_keys(&tx, &meta).unwrap();
        assert_eq!(keys[0], wallet.pubkey());
        let programs = executed_programs(&tx, &keys);
        assert_eq!(programs, vec![*PUMP_PROGRAM_ID]);
    }

    #[test]
    fn accounts_encoding_is_rejected_with_a_clear_message() {
        let err = account_keys(
            &EncodedTransaction::Accounts(UiAccountsList {
                signatures: vec![],
                account_keys: vec![],
            }),
            &meta_with(vec![], vec![], &[]),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");
    }

    #[test]
    fn json_encoded_transactions_also_decode() {
        let wallet = Keypair::new();
        let token = Pubkey::new_unique();
        let pre = vec![
            balance(1, *WSOL_MINT, 2_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 0, 6, wallet.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 1_000_000_000, 9, wallet.pubkey()),
            balance(2, token, 1_000_000, 6, wallet.pubkey()),
        ];
        // Same transaction, `json` encoding instead of base64.
        let header = MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        };
        let json_tx = EncodedTransaction::Json(UiTransaction {
            signatures: vec!["sig".to_string()],
            message: UiMessage::Raw(UiRawMessage {
                header,
                account_keys: vec![
                    wallet.pubkey().to_string(),
                    PUMP_PROGRAM_ID.to_string(),
                    TOKEN_PROGRAM.to_string(),
                ],
                recent_blockhash: Hash::default().to_string(),
                instructions: vec![UiCompiledInstruction {
                    program_id_index: 1,
                    accounts: vec![0],
                    data: String::new(),
                    stack_height: None,
                }],
                address_table_lookups: None,
            }),
        });
        let swap = decode_swap(
            &wallet.pubkey(),
            "sig9",
            108,
            None,
            &json_tx,
            &meta_with(pre, post, &[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(swap.venue, SwapVenue::PumpBondingCurve);
        assert_eq!(swap.programs, vec![PUMP_PROGRAM_ID.to_string()]);
        assert_eq!(swap.base_amount, 1_000_000);
    }

    #[test]
    fn json_parsed_encoding_is_rejected_with_a_clear_message() {
        let wallet = Keypair::new();
        let json_tx = EncodedTransaction::Json(UiTransaction {
            signatures: vec![],
            message: UiMessage::Parsed(UiParsedMessage {
                account_keys: vec![],
                recent_blockhash: String::new(),
                instructions: vec![],
                address_table_lookups: None,
            }),
        });
        let err = decode_swap(
            &wallet.pubkey(),
            "sig10",
            109,
            None,
            &json_tx,
            &meta_with(vec![], vec![], &[]),
        )
        .unwrap_err();
        assert!(err.to_string().contains("jsonParsed"), "{err}");
    }

    /// Build a real signed v0 transaction and return its wire JSON plus the
    /// signature. Providers serialise `EncodedTransaction` with the same
    /// serde types the RPC uses: a base64 subscription delivers the untagged
    /// `Binary` form — `["<base64 of the whole tx>", "base64"]` — which is
    /// exactly what `serde_json::to_value` produces here (verified against a
    /// live `getBlock` response on devnet).
    fn wire_tx(
        payer: &Pubkey,
        programs: &[Pubkey],
        keypair: &Keypair,
    ) -> (serde_json::Value, String) {
        let ixs: Vec<solana_sdk::instruction::Instruction> = programs
            .iter()
            .map(|p| solana_sdk::instruction::Instruction {
                program_id: *p,
                accounts: vec![solana_sdk::instruction::AccountMeta::new(*payer, true)],
                data: vec![1],
            })
            .collect();
        let msg = MessageV0::try_compile(payer, &ixs, &[], Hash::default()).unwrap();
        let tx = solana_sdk::transaction::VersionedTransaction::try_new(
            VersionedMessage::V0(msg),
            &[keypair],
        )
        .unwrap();
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap());
        let encoded = EncodedTransaction::Binary(
            b64,
            solana_transaction_status::TransactionBinaryEncoding::Base64,
        );
        (
            serde_json::to_value(&encoded).unwrap(),
            tx.signatures[0].to_string(),
        )
    }

    #[test]
    fn parses_a_base64_transaction_notification_end_to_end() {
        let whale = Keypair::new();
        let token = Pubkey::new_unique();
        let (wire_json, sig) = wire_tx(
            &whale.pubkey(),
            &[*PUMP_PROGRAM_ID, Pubkey::new_unique()],
            &whale,
        );

        // Whale spent 1 SOL for 12.345678 tokens on the pump curve.
        let pre = vec![
            balance(1, *WSOL_MINT, 5_000_000_000, 9, whale.pubkey()),
            balance(2, token, 0, 6, whale.pubkey()),
        ];
        let post = vec![
            balance(1, *WSOL_MINT, 4_000_000_000, 9, whale.pubkey()),
            balance(2, token, 12_345_678, 6, whale.pubkey()),
        ];
        let meta = meta_with(pre, post, &[]);
        let meta_json = serde_json::to_value(&meta).unwrap();

        let raw = serde_json::json!({
            "signature": sig,
            "slot": 4242,
            "blockTime": 1_700_000_123,
            "transaction": wire_json,
            "meta": meta_json,
        });

        let notif = parse_transaction_notification(&raw).expect("notification must parse");
        assert_eq!(notif.signature, sig);
        assert_eq!(notif.slot, 4242);
        assert_eq!(notif.block_time, Some(1_700_000_123));
        assert!(notif.succeeded());

        // The exact same pipeline the poll path runs, on the pushed payload.
        let swap = decode_swap(
            &whale.pubkey(),
            &notif.signature,
            notif.slot,
            notif.block_time,
            &notif.transaction,
            &notif.meta,
        )
        .unwrap()
        .expect("the notification must decode into a swap");
        assert_eq!(swap.side, Side::Buy);
        assert_eq!(swap.venue, SwapVenue::PumpBondingCurve);
        assert_eq!(swap.base_mint, token.to_string());
        assert_eq!(swap.quote_amount, 1_000_000_000);
        assert_eq!(swap.base_amount, 12_345_678);
        assert!(swap.succeeded);
    }

    #[test]
    fn notification_parser_flags_failed_transactions() {
        let whale = Keypair::new();
        let (wire_json, sig) = wire_tx(&whale.pubkey(), &[*PUMP_PROGRAM_ID], &whale);
        let mut meta = base_meta();
        meta.err = Some(solana_sdk::transaction::TransactionError::InstructionError(
            0,
            solana_sdk::instruction::InstructionError::Custom(1),
        ));
        let raw = serde_json::json!({
            "signature": sig,
            "slot": 7,
            "transaction": wire_json,
            "meta": serde_json::to_value(&meta).unwrap(),
        });
        let notif = parse_transaction_notification(&raw).expect("parses");
        assert!(!notif.succeeded(), "failed tx must be flagged for skipping");
        assert_eq!(notif.block_time, None, "absent blockTime stays None");
    }

    #[test]
    fn notification_parser_rejects_junk_instead_of_crashing() {
        // Missing everything.
        assert!(parse_transaction_notification(&serde_json::json!({})).is_none());
        // Empty signature.
        assert!(parse_transaction_notification(&serde_json::json!({
            "signature": "", "transaction": {}, "meta": {},
        }))
        .is_none());
        // Valid transaction envelope but unparseable meta (schema drift on
        // the provider side must not take the feed down).
        let whale = Keypair::new();
        let (wire_json, sig) = wire_tx(&whale.pubkey(), &[*PUMP_PROGRAM_ID], &whale);
        assert!(parse_transaction_notification(&serde_json::json!({
            "signature": sig,
            "slot": 1,
            "transaction": wire_json,
            "meta": { "err": "not-a-transaction-error" },
        }))
        .is_none());
    }

    #[test]
    fn decode_instructions_resolves_programs_accounts_and_data() {
        // Two instructions against two programs; each touches the payer and
        // one extra account, with distinct data so order is verifiable.
        let payer = Keypair::new();
        let prog_a = Pubkey::new_unique();
        let prog_b = Pubkey::new_unique();
        let extra = Pubkey::new_unique();
        let ixs = vec![
            solana_sdk::instruction::Instruction {
                program_id: prog_a,
                accounts: vec![
                    solana_sdk::instruction::AccountMeta::new(payer.pubkey(), true),
                    solana_sdk::instruction::AccountMeta::new_readonly(extra, false),
                ],
                data: vec![1, 2, 3],
            },
            solana_sdk::instruction::Instruction {
                program_id: prog_b,
                accounts: vec![solana_sdk::instruction::AccountMeta::new(
                    payer.pubkey(),
                    true,
                )],
                data: vec![9],
            },
        ];
        let msg = MessageV0::try_compile(&payer.pubkey(), &ixs, &[], Hash::default()).unwrap();
        let tx = solana_sdk::transaction::VersionedTransaction::try_new(
            VersionedMessage::V0(msg),
            &[&payer],
        )
        .unwrap();
        let bytes = bincode::serialize(&tx).unwrap();
        let encoded = EncodedTransaction::Binary(
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            solana_transaction_status::TransactionBinaryEncoding::Base64,
        );
        let decoded = decode_instructions(&encoded, &base_meta()).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].program_id, prog_a);
        assert_eq!(decoded[0].accounts, vec![payer.pubkey(), extra]);
        assert_eq!(decoded[0].data, vec![1, 2, 3]);
        assert_eq!(decoded[1].program_id, prog_b);
        assert_eq!(decoded[1].accounts, vec![payer.pubkey()]);
        assert_eq!(decoded[1].data, vec![9]);

        // The `json` (raw message) encoding resolves identically.
        let keys: Vec<String> = tx
            .message
            .static_account_keys()
            .iter()
            .map(|k| k.to_string())
            .collect();
        let raw_ixs: Vec<UiCompiledInstruction> = tx
            .message
            .instructions()
            .iter()
            .map(|ix| UiCompiledInstruction {
                program_id_index: ix.program_id_index,
                accounts: ix.accounts.clone(),
                data: bs58::encode(&ix.data).into_string(),
                stack_height: None,
            })
            .collect();
        let header = *tx.message.header();
        let json = EncodedTransaction::Json(UiTransaction {
            signatures: vec![tx.signatures[0].to_string()],
            message: UiMessage::Raw(UiRawMessage {
                header: MessageHeader {
                    num_required_signatures: header.num_required_signatures,
                    num_readonly_signed_accounts: header.num_readonly_signed_accounts,
                    num_readonly_unsigned_accounts: header.num_readonly_unsigned_accounts,
                },
                account_keys: keys,
                recent_blockhash: Hash::default().to_string(),
                instructions: raw_ixs,
                address_table_lookups: None,
            }),
        });
        let from_json = decode_instructions(&json, &base_meta()).unwrap();
        assert_eq!(from_json, decoded);
    }
}
