//! Jupiter aggregator — the routing fallback.
//!
//! Used in two places:
//!   * **Module 1 exits.** A graduated token may have moved to Raydium, Orca or
//!     a CLMM pool; Jupiter finds the route so the exit monitor does not need a
//!     pool registry per DEX.
//!   * **Module 2 mirrors.** When the copied trade went through Jupiter we
//!     cannot replay its exact route (it may be gone), so we re-quote and take
//!     whatever is best now.
//!
//! The v6 API is two calls: `GET /quote` for the price, then `POST /swap` which
//! returns a **base64 serialized, unsigned `VersionedTransaction`**. We
//! deserialize it, set our own blockhash only if needed, and sign it ourselves —
//! never trusting a remote signer.

use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use tracing::{debug, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::tokens::Wallet;

/// The current Jupiter quote endpoint.
pub const JUPITER_QUOTE_API: &str = "https://lite-api.jup.ag/swap/v1/quote";
/// The current Jupiter swap endpoint.
pub const JUPITER_SWAP_API: &str = "https://lite-api.jup.ag/swap/v1/swap";
/// The legacy v6 endpoints, kept because many operators pin them.
pub const JUPITER_V6_QUOTE: &str = "https://quote-api.jup.ag/v6/quote";
pub const JUPITER_V6_SWAP: &str = "https://quote-api.jup.ag/v6/swap";

/// A price quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JupiterQuote {
    pub input_mint: String,
    pub in_amount: String,
    pub output_mint: String,
    pub out_amount: String,
    pub other_amount_threshold: String,
    pub swap_mode: Option<String>,
    pub slippage_bps: Option<u64>,
    #[serde(default)]
    pub platform_fee: Option<PlatformFee>,
    pub price_impact_pct: Option<String>,
    #[serde(default)]
    pub route_plan: Vec<RouteStep>,
    /// Raw JSON, kept so an unexpected new field is still available.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformFee {
    pub amount: Option<String>,
    pub fee_bps: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStep {
    pub swap_info: Option<SwapInfo>,
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapInfo {
    pub amm_key: Option<String>,
    pub label: Option<String>,
    pub input_mint: Option<String>,
    pub output_mint: Option<String>,
    pub in_amount: Option<String>,
    pub out_amount: Option<String>,
    pub fee_amount: Option<String>,
    pub fee_mint: Option<String>,
}

impl JupiterQuote {
    pub fn out_amount_u64(&self) -> BotResult<u64> {
        self.out_amount
            .parse::<u64>()
            .map_err(|e| BotError::encoding(format!("jupiter out_amount: {e}")))
    }

    pub fn in_amount_u64(&self) -> BotResult<u64> {
        self.in_amount
            .parse::<u64>()
            .map_err(|e| BotError::encoding(format!("jupiter in_amount: {e}")))
    }

    pub fn threshold_u64(&self) -> BotResult<u64> {
        self.other_amount_threshold
            .parse::<u64>()
            .map_err(|e| BotError::encoding(format!("jupiter otherAmountThreshold: {e}")))
    }

    /// Price impact as a percentage (Jupiter returns it as a decimal string).
    pub fn price_impact_pct(&self) -> f64 {
        self.price_impact_pct
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|f| f * 100.0)
            .unwrap_or(0.0)
    }

    /// The DEXes this route passes through, in order.
    pub fn route_labels(&self) -> Vec<String> {
        self.route_plan
            .iter()
            .filter_map(|s| s.swap_info.as_ref().and_then(|i| i.label.clone()))
            .collect()
    }

    /// How many hops.
    pub fn hops(&self) -> usize {
        self.route_plan.len()
    }
}

/// Quote parameters.
#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    /// Raw units of the input mint.
    pub amount: u64,
    /// Slippage in basis points.
    pub slippage_bps: u64,
    /// `ExactIn` (default) or `ExactOut`.
    pub swap_mode: SwapMode,
    /// Restrict to these DEXes, e.g. `["Raydium","Pumpswap"]`.
    pub restrict_intermediate_tokens: bool,
    pub only_direct_routes: bool,
    /// Maximum hops; Jupiter's own default is 4.
    pub max_accounts: Option<usize>,
    /// Platform fee we take, in bps, and the wallet that receives it.
    pub platform_fee_bps: Option<u64>,
    pub platform_fee_account: Option<Pubkey>,
    /// Quote against a specific block height for consistency.
    pub max_accounts_for_one_hop: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SwapMode {
    ExactIn,
    ExactOut,
}

impl SwapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwapMode::ExactIn => "ExactIn",
            SwapMode::ExactOut => "ExactOut",
        }
    }
}

impl QuoteRequest {
    pub fn new(input_mint: Pubkey, output_mint: Pubkey, amount: u64) -> Self {
        QuoteRequest {
            input_mint,
            output_mint,
            amount,
            slippage_bps: 50,
            swap_mode: SwapMode::ExactIn,
            restrict_intermediate_tokens: true,
            only_direct_routes: false,
            max_accounts: None,
            platform_fee_bps: None,
            platform_fee_account: None,
            max_accounts_for_one_hop: None,
        }
    }

    pub fn slippage_bps(mut self, bps: u64) -> Self {
        self.slippage_bps = bps;
        self
    }

    pub fn direct_only(mut self) -> Self {
        self.only_direct_routes = true;
        self
    }

    pub fn platform_fee(mut self, bps: u64, account: Pubkey) -> Self {
        self.platform_fee_bps = Some(bps);
        self.platform_fee_account = Some(account);
        self
    }

    fn to_query(&self) -> Vec<(String, String)> {
        let mut q = vec![
            ("inputMint".to_string(), self.input_mint.to_string()),
            ("outputMint".to_string(), self.output_mint.to_string()),
            ("amount".to_string(), self.amount.to_string()),
            ("slippageBps".to_string(), self.slippage_bps.to_string()),
            ("swapMode".to_string(), self.swap_mode.as_str().to_string()),
            (
                "restrictIntermediateTokens".to_string(),
                self.restrict_intermediate_tokens.to_string(),
            ),
            (
                "onlyDirectRoutes".to_string(),
                self.only_direct_routes.to_string(),
            ),
        ];
        if let Some(n) = self.max_accounts {
            q.push(("maxAccounts".to_string(), n.to_string()));
        }
        if let Some(bps) = self.platform_fee_bps {
            q.push(("platformFeeBps".to_string(), bps.to_string()));
            if let Some(acc) = self.platform_fee_account {
                q.push(("feeAccount".to_string(), acc.to_string()));
            }
        }
        q
    }
}

/// Swap-instruction options sent to `POST /swap`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapRequest {
    pub user_public_key: String,
    pub quote_response: JupiterQuote,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_and_unwrap_sol: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_shared_accounts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_compute_unit_limit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_slippage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prioritization_fee_lamports: Option<PrioritizationFee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compute_unit_price_micro_lamports: Option<u64>,
}

/// Jupiter accepts either a flat tip or a Jito-style priority config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrioritizationFee {
    /// `{"priorityLevelWithMaxLamports":{"maxLamports":N,"priorityLevel":"high"}}`
    Level {
        #[serde(rename = "priorityLevelWithMaxLamports")]
        priority_level_with_max_lamports: PriorityLevelConfig,
    },
    /// A raw lamport amount, sent as `{"computeUnitPriceMicroLamports": N}`.
    Micros {
        #[serde(rename = "computeUnitPriceMicroLamports")]
        compute_unit_price_micro_lamports: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorityLevelConfig {
    pub max_lamports: u64,
    pub priority_level: String,
}

/// Response from `POST /swap`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapResponse {
    /// Base64 of an unsigned `VersionedTransaction`.
    pub swap_transaction: String,
    pub last_valid_block_height: Option<u64>,
    pub prioritization_fee_lamports: Option<u64>,
    pub compute_unit_limit: Option<u64>,
}

/// The client.
pub struct Jupiter {
    http: reqwest::Client,
    quote_url: String,
    swap_url: String,
}

impl Jupiter {
    /// Use the current `lite-api.jup.ag` endpoints.
    pub fn new() -> Self {
        Self::with_urls(JUPITER_QUOTE_API, JUPITER_SWAP_API)
    }

    pub fn with_urls(quote_url: impl Into<String>, swap_url: impl Into<String>) -> Self {
        Jupiter {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            quote_url: quote_url.into(),
            swap_url: swap_url.into(),
        }
    }

    pub fn quote_url(&self) -> &str {
        &self.quote_url
    }

    pub fn swap_url(&self) -> &str {
        &self.swap_url
    }

    /// `GET /quote`.
    pub async fn quote(&self, req: &QuoteRequest) -> BotResult<JupiterQuote> {
        let url = self.quote_url.clone();
        let query = req.to_query();
        debug!(
            input = %req.input_mint,
            output = %req.output_mint,
            amount = req.amount,
            "requesting jupiter quote"
        );
        let response = self
            .http
            .get(&url)
            .query(&query)
            .send()
            .await
            .map_err(|e| BotError::http(format!("jupiter quote: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("jupiter quote body: {e}")))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "jupiter quote http {status}: {}",
                truncate(&text, 300)
            )));
        }
        let quote: JupiterQuote = serde_json::from_str(&text)
            .map_err(|e| BotError::encoding(format!("jupiter quote json: {e} — {text}")))?;

        if quote.out_amount == "0" {
            return Err(BotError::solana(format!(
                "jupiter found no route for {} -> {} of {} (impact {})",
                req.input_mint,
                req.output_mint,
                req.amount,
                quote.price_impact_pct()
            )));
        }
        debug!(
            out = %quote.out_amount,
            hops = quote.hops(),
            impact = quote.price_impact_pct(),
            route = ?quote.route_labels(),
            "jupiter quote"
        );
        Ok(quote)
    }

    /// `POST /swap` — returns the unsigned transaction plus its validity.
    pub async fn swap_transaction(
        &self,
        user: &Pubkey,
        quote: JupiterQuote,
        compute_unit_price_micro_lamports: Option<u64>,
    ) -> BotResult<SwapResponse> {
        let body = SwapRequest {
            user_public_key: user.to_string(),
            quote_response: quote,
            wrap_and_unwrap_sol: Some(true),
            use_shared_accounts: Some(true),
            dynamic_compute_unit_limit: Some(true),
            dynamic_slippage: Some(true),
            prioritization_fee_lamports: None,
            compute_unit_price_micro_lamports,
        };
        let response = self
            .http
            .post(&self.swap_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BotError::http(format!("jupiter swap: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("jupiter swap body: {e}")))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "jupiter swap http {status}: {}",
                truncate(&text, 300)
            )));
        }
        serde_json::from_str(&text)
            .map_err(|e| BotError::encoding(format!("jupiter swap json: {e}")))
    }

    /// Deserialize Jupiter's transaction.
    ///
    /// The transaction is unsigned and carries Jupiter's blockhash. We verify
    /// the fee payer is *our* wallet before signing: a compromised or
    /// misconfigured aggregator response that set someone else as payer would
    /// otherwise be signed blindly.
    pub fn decode_transaction(
        response: &SwapResponse,
        expected_payer: &Pubkey,
    ) -> BotResult<VersionedTransaction> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(response.swap_transaction.trim())
            .map_err(|e| BotError::encoding(format!("jupiter tx base64: {e}")))?;
        let tx: VersionedTransaction = bincode::deserialize(&bytes)
            .map_err(|e| BotError::encoding(format!("jupiter tx bincode: {e}")))?;

        let payer = tx.message.static_account_keys().first().copied();
        if payer != Some(*expected_payer) {
            return Err(BotError::signing(format!(
                "jupiter returned a transaction whose fee payer is {:?}, not {expected_payer} — refusing to sign",
                payer
            )));
        }
        if !tx.signatures.is_empty() && tx.signatures.iter().any(|s| *s != Signature::default()) {
            warn!("jupiter returned a pre-signed transaction; we will re-sign it anyway");
        }
        Ok(tx)
    }

    /// Sign Jupiter's transaction with our wallet.
    pub fn sign(response: &SwapResponse, wallet: &Wallet) -> BotResult<VersionedTransaction> {
        let mut tx = Self::decode_transaction(response, &wallet.pubkey)?;
        let message_bytes = tx.message.serialize();
        let signature = wallet.sign_message_sync(&message_bytes);
        tx.signatures = vec![signature];
        Ok(tx)
    }

    /// Replace the blockhash Jupiter chose with a fresh one of ours, then sign.
    ///
    /// Jupiter's `lastValidBlockHeight` can already be close to expiry by the
    /// time the response arrives, which shows up as `BlockhashNotFound`.
    pub fn resign_with_blockhash(
        response: &SwapResponse,
        wallet: &Wallet,
        blockhash: Hash,
    ) -> BotResult<VersionedTransaction> {
        let mut tx = Self::decode_transaction(response, &wallet.pubkey)?;
        set_blockhash(&mut tx, blockhash)?;
        let message_bytes = tx.message.serialize();
        let signature = wallet.sign_message_sync(&message_bytes);
        tx.signatures = vec![signature];
        Ok(tx)
    }

    /// Quote then build a signed transaction in one call.
    pub async fn build_swap(
        &self,
        wallet: &Wallet,
        req: &QuoteRequest,
        blockhash: Option<Hash>,
        compute_unit_price_micro_lamports: Option<u64>,
    ) -> BotResult<(JupiterQuote, VersionedTransaction, Option<u64>)> {
        let quote = self.quote(req).await?;
        let response = self
            .swap_transaction(
                &wallet.pubkey,
                quote.clone(),
                compute_unit_price_micro_lamports,
            )
            .await?;
        let last_valid = response.last_valid_block_height;
        let tx = match blockhash {
            Some(bh) => Self::resign_with_blockhash(&response, wallet, bh)?,
            None => Self::sign(&response, wallet)?,
        };
        Ok((quote, tx, last_valid))
    }

    /// Convenience: sell `amount` of `mint` into WSOL.
    pub async fn quote_sell_to_sol(
        &self,
        mint: &Pubkey,
        amount: u64,
        slippage_bps: u64,
    ) -> BotResult<JupiterQuote> {
        self.quote(&QuoteRequest::new(*mint, *WSOL_MINT, amount).slippage_bps(slippage_bps))
            .await
    }

    /// Convenience: buy `mint` with `lamports` of SOL.
    pub async fn quote_buy_with_sol(
        &self,
        mint: &Pubkey,
        lamports: u64,
        slippage_bps: u64,
    ) -> BotResult<JupiterQuote> {
        self.quote(&QuoteRequest::new(*WSOL_MINT, *mint, lamports).slippage_bps(slippage_bps))
            .await
    }
}

impl Default for Jupiter {
    fn default() -> Self {
        Jupiter::new()
    }
}

/// Rewrite the blockhash inside an already-built message.
///
/// `VersionedMessage` stores it as a `Hash` in both the legacy and v0 variants,
/// so this is a field write rather than a recompile — the account list and
/// instruction data are untouched.
pub fn set_blockhash(tx: &mut VersionedTransaction, blockhash: Hash) -> BotResult<()> {
    use solana_sdk::message::VersionedMessage;
    match &mut tx.message {
        VersionedMessage::V0(m) => m.recent_blockhash = blockhash,
        VersionedMessage::Legacy(m) => m.recent_blockhash = blockhash,
    }
    Ok(())
}

/// The blockhash currently inside a transaction.
pub fn blockhash_of(tx: &VersionedTransaction) -> Hash {
    use solana_sdk::message::VersionedMessage;
    match &tx.message {
        VersionedMessage::V0(m) => m.recent_blockhash,
        VersionedMessage::Legacy(m) => m.recent_blockhash,
    }
}

/// Summary for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterQuoteSummary {
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: u64,
    pub out_amount: u64,
    pub slippage_bps: u64,
    pub price_impact_pct: f64,
    pub route: Vec<String>,
}

impl JupiterQuoteSummary {
    pub fn new(quote: &JupiterQuote) -> Self {
        JupiterQuoteSummary {
            input_mint: quote.input_mint.clone(),
            output_mint: quote.output_mint.clone(),
            in_amount: quote.in_amount.parse().unwrap_or(0),
            out_amount: quote.out_amount.parse().unwrap_or(0),
            slippage_bps: quote.slippage_bps.unwrap_or(0),
            price_impact_pct: quote.price_impact_pct(),
            route: quote.route_labels(),
        }
    }

    /// Human-readable price of the input in terms of the output.
    pub fn price(&self, in_decimals: u8, out_decimals: u8) -> f64 {
        let i = maths::from_raw_amount(self.in_amount, in_decimals);
        if i == 0.0 {
            return 0.0;
        }
        maths::from_raw_amount(self.out_amount, out_decimals) / i
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = n;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::v0::Message as MessageV0;
    use solana_sdk::message::VersionedMessage;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    use solana_system_interface::instruction as system_instruction;

    fn quote_json(out: &str, impact: &str) -> String {
        format!(
            r#"{{
                "inputMint": "So11111111111111111111111111111111111111112",
                "inAmount": "1000000000",
                "outputMint": "TokenMint1111111111111111111111111111111111",
                "outAmount": "{out}",
                "otherAmountThreshold": "9500000",
                "swapMode": "ExactIn",
                "slippageBps": 50,
                "priceImpactPct": "{impact}",
                "routePlan": [
                    {{"swapInfo": {{"ammKey":"a","label":"Pumpswap","inputMint":"x","outputMint":"y","inAmount":"1","outAmount":"2","feeAmount":"0","feeMint":"z"}}, "percent": 100}}
                ]
            }}"#
        )
    }

    #[test]
    fn quote_parses_and_derives_values() {
        let q: JupiterQuote = serde_json::from_str(&quote_json("10000000", "0.0125")).unwrap();
        assert_eq!(q.out_amount_u64().unwrap(), 10_000_000);
        assert_eq!(q.in_amount_u64().unwrap(), 1_000_000_000);
        assert_eq!(q.threshold_u64().unwrap(), 9_500_000);
        assert!(
            (q.price_impact_pct() - 1.25).abs() < 1e-9,
            "0.0125 => 1.25%"
        );
        assert_eq!(q.hops(), 1);
        assert_eq!(q.route_labels(), vec!["Pumpswap".to_string()]);
        assert_eq!(q.slippage_bps, Some(50));
    }

    #[test]
    fn quote_tolerates_missing_optional_fields() {
        let minimal = r#"{"inputMint":"a","inAmount":"1","outputMint":"b","outAmount":"2","otherAmountThreshold":"1"}"#;
        let q: JupiterQuote = serde_json::from_str(minimal).unwrap();
        assert_eq!(q.out_amount_u64().unwrap(), 2);
        assert_eq!(q.price_impact_pct(), 0.0);
        assert!(q.route_labels().is_empty());
        assert!(q.platform_fee.is_none());
    }

    #[test]
    fn quote_rejects_a_malformed_amount() {
        let q: JupiterQuote = serde_json::from_str(&quote_json("not-a-number", "0")).unwrap();
        assert!(q.out_amount_u64().is_err());
    }

    #[test]
    fn quote_request_builds_the_expected_query() {
        let req = QuoteRequest::new(*WSOL_MINT, Pubkey::new_unique(), 1_000)
            .slippage_bps(120)
            .direct_only()
            .platform_fee(10, Pubkey::new_unique());
        let q = req.to_query();
        let get = |k: &str| q.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get("amount").as_deref(), Some("1000"));
        assert_eq!(get("slippageBps").as_deref(), Some("120"));
        assert_eq!(get("swapMode").as_deref(), Some("ExactIn"));
        assert_eq!(get("onlyDirectRoutes").as_deref(), Some("true"));
        assert_eq!(get("platformFeeBps").as_deref(), Some("10"));
        assert!(get("feeAccount").is_some());
        assert!(get("maxAccounts").is_none());
    }

    #[test]
    fn decode_transaction_refuses_a_foreign_fee_payer() {
        let attacker = Keypair::new();
        let victim = Keypair::new();
        let msg = MessageV0::try_compile(
            &attacker.pubkey(),
            &[system_instruction::transfer(
                &attacker.pubkey(),
                &Pubkey::new_unique(),
                1,
            )],
            &[],
            Hash::default(),
        )
        .unwrap();
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[&attacker]).unwrap();
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap());
        let response = SwapResponse {
            swap_transaction: b64,
            last_valid_block_height: Some(10),
            prioritization_fee_lamports: None,
            compute_unit_limit: None,
        };
        let err = Jupiter::decode_transaction(&response, &victim.pubkey()).unwrap_err();
        assert!(err.to_string().contains("refusing to sign"), "{err}");
    }

    #[test]
    fn sign_produces_a_verifiable_signature() {
        let wallet_kp = Keypair::new();
        let wallet = Wallet::load(&bs58::encode(wallet_kp.to_bytes()).into_string()).unwrap();
        assert_eq!(wallet.pubkey, wallet_kp.pubkey());

        let msg = MessageV0::try_compile(
            &wallet.pubkey,
            &[system_instruction::transfer(
                &wallet.pubkey,
                &Pubkey::new_unique(),
                1,
            )],
            &[],
            Hash::new_unique(),
        )
        .unwrap();
        let unsigned = VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::V0(msg),
        };
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(bincode::serialize(&unsigned).unwrap());
        let response = SwapResponse {
            swap_transaction: b64,
            last_valid_block_height: None,
            prioritization_fee_lamports: None,
            compute_unit_limit: None,
        };

        let signed = Jupiter::sign(&response, &wallet).unwrap();
        assert_eq!(signed.signatures.len(), 1);
        assert_ne!(signed.signatures[0], Signature::default());
        // The signature must verify against the serialized message with our key.
        use ed25519_dalek::Verifier;
        let msg = signed.message.serialize();
        let sig_bytes: [u8; 64] = signed.signatures[0].as_ref().try_into().unwrap();
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&wallet.pubkey.to_bytes()).unwrap();
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        assert!(
            vk.verify(&msg, &sig).is_ok(),
            "our own signature must verify"
        );
    }

    #[test]
    fn resigning_swaps_the_blockhash_without_touching_the_message() {
        let wallet_kp = Keypair::new();
        let wallet = Wallet::load(&bs58::encode(wallet_kp.to_bytes()).into_string()).unwrap();
        let original = Hash::new_unique();
        let msg = MessageV0::try_compile(
            &wallet.pubkey,
            &[system_instruction::transfer(
                &wallet.pubkey,
                &Pubkey::new_unique(),
                1,
            )],
            &[],
            original,
        )
        .unwrap();
        let account_count = msg.account_keys.len();
        let tx = VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::V0(msg),
        };
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap());
        let response = SwapResponse {
            swap_transaction: b64,
            last_valid_block_height: None,
            prioritization_fee_lamports: None,
            compute_unit_limit: None,
        };

        let fresh = Hash::new_unique();
        let resigned = Jupiter::resign_with_blockhash(&response, &wallet, fresh).unwrap();
        assert_eq!(blockhash_of(&resigned), fresh);
        assert_eq!(resigned.message.static_account_keys().len(), account_count);
    }

    #[test]
    fn set_blockhash_handles_legacy_messages_too() {
        let kp = Keypair::new();
        let legacy = solana_sdk::message::Message::new(
            &[system_instruction::transfer(
                &kp.pubkey(),
                &Pubkey::new_unique(),
                1,
            )],
            Some(&kp.pubkey()),
        );
        let mut tx = VersionedTransaction {
            signatures: vec![Signature::default()],
            message: VersionedMessage::Legacy(legacy),
        };
        let bh = Hash::new_unique();
        set_blockhash(&mut tx, bh).unwrap();
        assert_eq!(blockhash_of(&tx), bh);
    }

    #[test]
    fn decode_rejects_garbage() {
        let response = SwapResponse {
            swap_transaction: "!!!not base64".into(),
            last_valid_block_height: None,
            prioritization_fee_lamports: None,
            compute_unit_limit: None,
        };
        assert!(Jupiter::decode_transaction(&response, &Pubkey::new_unique()).is_err());

        let valid_b64_but_not_a_tx = SwapResponse {
            swap_transaction: base64::engine::general_purpose::STANDARD.encode(b"hello world"),
            last_valid_block_height: None,
            prioritization_fee_lamports: None,
            compute_unit_limit: None,
        };
        assert!(
            Jupiter::decode_transaction(&valid_b64_but_not_a_tx, &Pubkey::new_unique()).is_err()
        );
    }

    #[test]
    fn summary_price_normalises_decimals() {
        let q: JupiterQuote = serde_json::from_str(&quote_json("10000000", "0.01")).unwrap();
        let s = JupiterQuoteSummary::new(&q);
        assert_eq!(s.out_amount, 10_000_000);
        assert_eq!(s.route, vec!["Pumpswap".to_string()]);
        // 1 SOL (9dp) in, 10 tokens (6dp) out => 10 tokens per SOL.
        let p = s.price(9, 6);
        assert!((p - 10.0).abs() < 1e-9, "got {p}");
    }

    #[test]
    fn default_client_uses_the_current_endpoints() {
        let j = Jupiter::new();
        assert!(j.quote_url().contains("jup.ag"));
        assert!(j.swap_url().contains("/swap"));
        assert_eq!(SwapMode::ExactOut.as_str(), "ExactOut");
    }

    #[test]
    fn prioritization_fee_serialises_either_way() {
        let level = PrioritizationFee::Level {
            priority_level_with_max_lamports: PriorityLevelConfig {
                max_lamports: 500_000,
                priority_level: "high".into(),
            },
        };
        let json = serde_json::to_value(&level).unwrap();
        assert_eq!(
            json["priorityLevelWithMaxLamports"]["priorityLevel"],
            "high"
        );

        let micros = PrioritizationFee::Micros {
            compute_unit_price_micro_lamports: 1000,
        };
        let json = serde_json::to_value(&micros).unwrap();
        assert_eq!(json["computeUnitPriceMicroLamports"], 1000);
    }

    #[test]
    fn swap_request_omits_unset_prioritization() {
        let q: JupiterQuote = serde_json::from_str(&quote_json("1", "0")).unwrap();
        let req = SwapRequest {
            user_public_key: Pubkey::new_unique().to_string(),
            quote_response: q,
            wrap_and_unwrap_sol: Some(true),
            use_shared_accounts: None,
            dynamic_compute_unit_limit: None,
            dynamic_slippage: None,
            prioritization_fee_lamports: None,
            compute_unit_price_micro_lamports: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("useSharedAccounts").is_none());
        assert!(json.get("prioritizationFeeLamports").is_none());
        assert_eq!(json["wrapAndUnwrapSol"], true);
        assert!(json.get("quoteResponse").is_some());
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert!(truncate("日本語", 4).ends_with('…'));
    }
}
