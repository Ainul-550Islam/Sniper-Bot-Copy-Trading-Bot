//! Wallet handling: keypair loading from every common format, balances, ATA
//! management and WSOL wrapping.

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_system_interface::instruction as system_instruction;
use spl_associated_token_account::instruction::create_associated_token_account_idempotent;
use tracing::{debug, info, warn};

use bot_core::error::{BotError, BotResult};
use bot_core::maths;

use crate::consts::*;
use crate::rpc::Rpc;
use crate::signer::TransactionSigner;

use bot_core::error::SignerError;

/// The trading wallet plus cached on-chain facts about it.
pub struct Wallet {
    keypair: Keypair,
    pub pubkey: Pubkey,
    /// How the keypair was loaded, for the startup log line.
    pub source: String,
}

/// Hand-written: public key and load-source only. The derived `Debug` would
/// print the `Keypair`, which `Debug`s its secret bytes.
impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("pubkey", &self.pubkey.to_string())
            .field("source", &self.source)
            .finish()
    }
}

#[async_trait]
impl TransactionSigner for Wallet {
    fn pubkey(&self) -> Pubkey {
        self.pubkey
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        Ok(self.sign_message_sync(message))
    }
}

impl Wallet {
    /// Sign message bytes with the wallet key (synchronous local signing).
    ///
    /// This is the ONLY place in the workspace that reaches the private key
    /// after loading: everything else goes through the
    /// [`TransactionSigner`] boundary. Keypair material is never logged,
    /// never returned and never serialized by this type.
    pub fn sign_message_sync(&self, message: &[u8]) -> Signature {
        self.keypair.sign_message(message)
    }

    /// Load a keypair from any of the formats people actually use:
    ///
    /// * a path to a `solana-keygen` JSON array file (`[12,34,…]`)
    /// * a path to a file containing a base58 secret key
    /// * an inline base58 secret key (32 or 64 bytes)
    /// * an inline JSON array
    ///
    /// The value comes from the environment (`SOLANA_KEYPAIR`) and is never
    /// logged.
    pub fn load(spec: &str) -> BotResult<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(BotError::config("keypair spec is empty"));
        }

        // 1. Inline JSON array.
        if spec.starts_with('[') {
            let bytes = parse_json_array(spec).map_err(|e| {
                BotError::config(format!("keypair is not a valid JSON byte array: {e}"))
            })?;
            return Self::from_bytes(&bytes, "inline json array");
        }

        // 2. A filesystem path.
        let path = Path::new(spec);
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| BotError::config(format!("cannot read keypair file {spec}: {e}")))?;
            let text = text.trim();
            let source = format!("file {}", path.display());
            if text.starts_with('[') {
                let bytes = parse_json_array(text).map_err(|e| {
                    BotError::config(format!("keypair file is not a valid JSON byte array: {e}"))
                })?;
                return Self::from_bytes(&bytes, &source);
            }
            // Otherwise treat the file contents as a base58 secret key.
            let bytes = bs58::decode(text)
                .into_vec()
                .map_err(|e| BotError::config(format!("keypair file is not valid base58: {e}")))?;
            return Self::from_bytes(&bytes, &source);
        }

        // 3. Inline base58.
        let bytes = bs58::decode(spec)
            .into_vec()
            .map_err(|e| BotError::config(format!("keypair is not valid base58: {e}")))?;
        Self::from_bytes(&bytes, "inline base58")
    }

    fn from_bytes(bytes: &[u8], source: &str) -> BotResult<Self> {
        // `Keypair::try_from` wants the full 64-byte ed25519 keypair; a
        // 32-byte seed also works via the deterministic expansion below.
        let keypair = match bytes.len() {
            64 => Keypair::try_from(bytes)
                .map_err(|e| BotError::config(format!("invalid 64-byte keypair: {e}")))?,
            32 => {
                // A raw seed: expand it deterministically.
                let mut full = [0u8; 64];
                full[..32].copy_from_slice(bytes);
                let kp = Keypair::try_from(full.as_slice()).ok();
                match kp {
                    Some(k) => k,
                    None => {
                        // Fall back to deriving the public half from the seed.
                        use ed25519_dalek::SigningKey;
                        let mut seed = [0u8; 32];
                        seed.copy_from_slice(bytes);
                        let signing = SigningKey::from_bytes(&seed);
                        let mut full = [0u8; 64];
                        full[..32].copy_from_slice(&seed);
                        full[32..].copy_from_slice(signing.verifying_key().as_bytes());
                        Keypair::try_from(full.as_slice()).map_err(|e| {
                            BotError::config(format!("invalid 32-byte keypair seed: {e}"))
                        })?
                    }
                }
            }
            n => {
                return Err(BotError::config(format!(
                    "keypair must be 32 or 64 bytes, got {n}"
                )))
            }
        };
        let pubkey = keypair.pubkey();
        info!(%pubkey, source, "loaded solana wallet");
        Ok(Wallet {
            keypair,
            pubkey,
            source: source.to_string(),
        })
    }

    /// Wrap an already-constructed keypair (used by [`Wallet::generate`] and
    /// by tests / tooling that derived a keypair elsewhere).
    pub fn from_keypair(keypair: Keypair) -> Self {
        let pubkey = keypair.pubkey();
        Wallet {
            keypair,
            pubkey,
            source: "in-memory keypair".into(),
        }
    }

    /// Generate a fresh wallet (useful for devnet/paper runs).
    pub fn generate() -> Self {
        let wallet = Self::from_keypair(Keypair::new());
        warn!(pubkey = %wallet.pubkey, "generated an ephemeral wallet — it holds no funds");
        Wallet {
            source: "generated".into(),
            ..wallet
        }
    }

    pub async fn sol_balance(&self, rpc: &Rpc) -> BotResult<f64> {
        let lamports = rpc.get_balance(&self.pubkey).await?;
        Ok(maths::lamports_to_sol(lamports))
    }

    pub async fn lamports(&self, rpc: &Rpc) -> BotResult<u64> {
        rpc.get_balance(&self.pubkey).await
    }

    /// ATA for `mint`, created idempotently if missing.
    ///
    /// Returns `(address, Some(instruction))` when the account has to be
    /// created in the same transaction, or `(address, None)` when it exists.
    pub async fn ensure_ata(
        &self,
        rpc: &Rpc,
        mint: &Pubkey,
    ) -> BotResult<(Pubkey, Option<Instruction>)> {
        let token_program = rpc.token_program_of(mint).await.unwrap_or(*TOKEN_PROGRAM);
        let ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            &self.pubkey,
            mint,
            &token_program,
        );
        if rpc.account_exists(&ata).await.unwrap_or(false) {
            return Ok((ata, None));
        }
        debug!(%mint, %ata, "creating associated token account");
        let ix = create_associated_token_account_idempotent(
            &self.pubkey,
            &self.pubkey,
            mint,
            &token_program,
        );
        Ok((ata, Some(ix)))
    }

    /// Wrap SOL into the wSOL ATA so it can be used as a swap input.
    ///
    /// Returns the instructions needed: create the ATA if absent, transfer the
    /// SOL in, then `sync_native` so the token program sees the new balance.
    pub async fn wrap_sol_instructions(
        &self,
        rpc: &Rpc,
        lamports: u64,
    ) -> BotResult<(Pubkey, Vec<Instruction>)> {
        let (ata, create_ix) = self.ensure_ata(rpc, &WSOL_MINT).await?;
        let mut ixs = Vec::new();
        if let Some(ix) = create_ix {
            ixs.push(ix);
        }
        ixs.push(system_instruction::transfer(&self.pubkey, &ata, lamports));
        ixs.push(
            spl_token::instruction::sync_native(&TOKEN_PROGRAM, &ata)
                .map_err(|e| BotError::solana(format!("sync_native: {e}")))?,
        );
        Ok((ata, ixs))
    }

    /// Unwrap wSOL back to SOL by closing the ATA.
    pub async fn unwrap_sol_instructions(&self, rpc: &Rpc) -> BotResult<Vec<Instruction>> {
        let (ata, _create) = self.ensure_ata(rpc, &WSOL_MINT).await?;
        Ok(vec![spl_token::instruction::close_account(
            &TOKEN_PROGRAM,
            &ata,
            &self.pubkey,
            &self.pubkey,
            &[],
        )
        .map_err(|e| {
            BotError::solana(format!("close wsol account: {e}"))
        })?])
    }

    /// Token balance of an ATA, or 0 when the account does not exist.
    pub async fn token_balance(
        &self,
        rpc: &Rpc,
        mint: &Pubkey,
        token_program: &Pubkey,
    ) -> BotResult<u64> {
        let ata = spl_associated_token_account::get_associated_token_address_with_program_id(
            &self.pubkey,
            mint,
            token_program,
        );
        let data = match rpc.get_account_processed(&ata).await? {
            Some(d) => d,
            None => return Ok(0),
        };
        // SPL TokenAccount layout: mint(32) owner(32) amount(8) …
        if data.len() < 72 {
            return Err(BotError::solana(format!(
                "token account data too short: {} bytes",
                data.len()
            )));
        }
        let mut amount = [0u8; 8];
        amount.copy_from_slice(&data[64..72]);
        Ok(u64::from_le_bytes(amount))
    }

    /// Compute-budget instructions placed at the front of every transaction.
    pub fn budget_instructions(
        compute_unit_limit: u32,
        priority_fee_micro_lamports: u64,
    ) -> Vec<Instruction> {
        let mut ixs = Vec::with_capacity(2);
        if compute_unit_limit > 0 {
            ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
                compute_unit_limit,
            ));
        }
        if priority_fee_micro_lamports > 0 {
            ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
                priority_fee_micro_lamports,
            ));
        }
        ixs
    }
}

fn parse_json_array(text: &str) -> Result<Vec<u8>, String> {
    let values: Vec<serde_json::Value> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    values
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| format!("not a byte: {v}"))
        })
        .collect()
}

/// Resolve a pubkey from a config string, with a clear error message.
pub fn parse_pubkey(label: &str, value: &str) -> BotResult<Pubkey> {
    Pubkey::from_str(value.trim())
        .map_err(|e| BotError::InvalidPubkey(format!("{label} = '{value}': {e}")))
}

/// Resolve an optional pubkey from config.
pub fn parse_pubkey_opt(label: &str, value: Option<&String>) -> BotResult<Option<Pubkey>> {
    match value {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => Ok(Some(parse_pubkey(label, v)?)),
    }
}

/// Aggregated SPL balance reading for one owner/mint pair (§D/§O).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenBalanceReading {
    /// Sum of raw integer amounts across every validated token account.
    pub raw_total: u64,
    /// `raw_total / 10^decimals`.
    pub ui_total: f64,
    /// Mint decimals as reported by the chain.
    pub decimals: u8,
    /// Number of token accounts that contributed (0 = owner holds none).
    pub accounts: usize,
}

/// Sum ALL of `owner`'s token accounts for `mint` — the ATA plus any
/// auxiliary accounts — so balances are never undercounted when tokens sit
/// outside the associated account.
///
/// Identity is validated per account (mint AND owner fields must match the
/// request), which rules out counting unrelated or spoofed accounts, and
/// multi-account aggregation cannot double-count because each on-chain
/// account contributes exactly once.
///
/// Contract (§O): `Ok` with `accounts == 0` and `ui_total == 0.0` means the
/// chain *answered* that there is no balance ("no balance"); `Err` means the
/// balance could not be read ("unreadable"). Callers must never collapse the
/// two — reconciliation treats `Err` as retry, never as zero.
///
/// Handles both wire shapes the RPC may return for account data: the
/// `jsonParsed` object and raw `base64` (decoded with the SPL Token
/// `Account` layout: mint(32) owner(32) amount(8)).
pub async fn token_balances_for_owner(
    rpc: &Rpc,
    owner: &Pubkey,
    mint: &Pubkey,
) -> BotResult<TokenBalanceReading> {
    use base64::Engine as _;
    let filter = solana_client::rpc_request::TokenAccountsFilter::Mint(*mint);
    let accounts = rpc
        .raw()
        .get_token_accounts_by_owner(owner, filter)
        .await
        .map_err(|e| BotError::solana(format!("token accounts read failed: {e}")))?;

    let mut raw_total: u64 = 0;
    let mut decimals: Option<u8> = None;
    let mut counted = 0usize;
    let owner_s = owner.to_string();
    let mint_s = mint.to_string();

    for a in accounts {
        // `UiAccountData` is serde-untagged: jsonParsed serialises to the
        // parsed object, base64 to `["<b64>", "base64"]`.
        let v = match serde_json::to_value(&a.account.data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(info) = v.pointer("/parsed/info") {
            let acct_mint = info.get("mint").and_then(|m| m.as_str()).unwrap_or("");
            let acct_owner = info.get("owner").and_then(|o| o.as_str()).unwrap_or("");
            if acct_mint != mint_s || acct_owner != owner_s {
                // Filtered by the RPC already; belt and braces so a buggy or
                // hostile endpoint can never inflate the balance.
                warn!(acct_mint, acct_owner, %mint_s, %owner_s, "token account identity mismatch — skipped");
                continue;
            }
            let amount = info
                .pointer("/tokenAmount/amount")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<u64>().ok());
            let dec = info
                .pointer("/tokenAmount/decimals")
                .and_then(|x| x.as_u64())
                .map(|d| d.min(32) as u8);
            if let (Some(amount), Some(dec)) = (amount, dec) {
                raw_total = raw_total.saturating_add(amount);
                decimals = Some(dec);
                counted += 1;
            }
            continue;
        }
        if let Some(arr) = v.as_array() {
            if arr.len() == 2 && arr[1].as_str() == Some("base64") {
                if let Some(b64) = arr[0].as_str() {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                        // SPL Token `Account`: mint(32) owner(32) amount(8)…
                        if bytes.len() >= 72
                            && bytes[0..32] == mint.to_bytes()
                            && bytes[32..64] == owner.to_bytes()
                        {
                            let mut amount = [0u8; 8];
                            amount.copy_from_slice(&bytes[64..72]);
                            raw_total = raw_total.saturating_add(u64::from_le_bytes(amount));
                            counted += 1;
                        }
                    }
                }
            }
        }
    }

    let decimals = match decimals {
        Some(d) => d,
        // Raw-shaped accounts carry no decimals; ask the chain once.
        None if counted > 0 => rpc.token_decimals(mint).await?,
        None => 0,
    };
    let ui_total = raw_total as f64 / 10f64.powi(i32::from(decimals));
    Ok(TokenBalanceReading {
        raw_total,
        ui_total,
        decimals,
        accounts: counted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_an_inline_base58_keypair() {
        let kp = Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = Wallet::load(&b58).expect("base58 keypair must load");
        assert_eq!(wallet.pubkey, kp.pubkey());
        assert_eq!(wallet.source, "inline base58");
    }

    #[test]
    fn loads_an_inline_json_array_keypair() {
        let kp = Keypair::new();
        let json = serde_json::to_string(&kp.to_bytes().to_vec()).unwrap();
        let wallet = Wallet::load(&json).expect("json array keypair must load");
        assert_eq!(wallet.pubkey, kp.pubkey());
        assert_eq!(wallet.source, "inline json array");
    }

    #[test]
    fn loads_a_keypair_from_a_json_file() {
        let kp = Keypair::new();
        let dir = std::env::temp_dir().join(format!("wallet-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("id.json");
        std::fs::write(
            &path,
            serde_json::to_string(&kp.to_bytes().to_vec()).unwrap(),
        )
        .unwrap();

        let wallet = Wallet::load(path.to_str().unwrap()).expect("file keypair must load");
        assert_eq!(wallet.pubkey, kp.pubkey());
        assert!(wallet.source.starts_with("file "));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loads_a_32_byte_seed() {
        let seed = [7u8; 32];
        let b58 = bs58::encode(seed).into_string();
        let wallet = Wallet::load(&b58).expect("32-byte seed must load");
        // Loading the same seed twice must give the same pubkey (deterministic).
        let again = Wallet::load(&b58).unwrap();
        assert_eq!(wallet.pubkey, again.pubkey);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Wallet::load("").is_err());
        assert!(Wallet::load("not a keypair at all!!!").is_err());
        assert!(Wallet::load("[1,2,3]").is_err(), "3 bytes is not a keypair");
        assert!(Wallet::load("[999,1]").is_err(), "999 is not a byte");
    }

    #[test]
    fn generated_wallet_is_usable() {
        let w = Wallet::generate();
        assert_eq!(w.source, "generated");
        let sig = w.sign_message_sync(b"usable");
        assert!(sig.verify(w.pubkey.as_ref(), b"usable"));
    }

    #[test]
    fn budget_instructions_follow_the_config() {
        assert!(Wallet::budget_instructions(0, 0).is_empty());
        let only_limit = Wallet::budget_instructions(200_000, 0);
        assert_eq!(only_limit.len(), 1);
        let both = Wallet::budget_instructions(200_000, 1_000);
        assert_eq!(both.len(), 2);
        // The limit instruction must come first: compute-budget instructions
        // are order-insensitive but convention puts the limit ahead of the price.
        assert_eq!(both[0].program_id, *COMPUTE_BUDGET_PROGRAM);
        assert_eq!(both[0].data[0], 2, "SetComputeUnitLimit tag is 2");
        assert_eq!(both[1].data[0], 3, "SetComputeUnitPrice tag is 3");
    }

    #[test]
    fn pubkey_parsing_reports_the_label() {
        assert!(parse_pubkey("fee_treasury", "garbage").is_err());
        let err = parse_pubkey("fee_treasury", "garbage")
            .unwrap_err()
            .to_string();
        assert!(err.contains("fee_treasury"), "{err}");

        assert!(parse_pubkey_opt("x", None).unwrap().is_none());
        assert!(parse_pubkey_opt("x", Some(&String::new()))
            .unwrap()
            .is_none());
        let pk = Pubkey::new_unique();
        assert_eq!(
            parse_pubkey_opt("x", Some(&pk.to_string())).unwrap(),
            Some(pk)
        );
    }

    #[test]
    fn wsol_wrap_instructions_are_ordered_correctly() {
        // Instruction shape is checked without a network: transfer then sync.
        let wallet = Pubkey::new_unique();
        let ata = Pubkey::new_unique();
        let transfer = system_instruction::transfer(&wallet, &ata, 1_000);
        let sync = spl_token::instruction::sync_native(&TOKEN_PROGRAM, &ata).unwrap();
        assert_eq!(transfer.program_id, *SYSTEM_PROGRAM);
        assert_eq!(sync.program_id, *TOKEN_PROGRAM);
        assert_eq!(sync.data[0], 17, "SyncNative tag is 17");
    }
}
