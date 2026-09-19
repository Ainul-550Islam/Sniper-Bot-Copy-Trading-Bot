//! Self-healing account-layout templates.
//!
//! Pump.fun has changed the account list its `buy`/`sell` instructions require
//! several times *without notice*: first the volume accumulators, then
//! `fee_config` + `fee_program`, then `bonding-curve-v2` as part of the
//! cashback upgrade. Each change produced a confusing on-chain failure
//! (`custom program error 6024 "Overflow"`) for every bot that hardcoded the
//! old list.
//!
//! This module stores an [`AccountLayout`] template per instruction that can
//! be:
//!   1. derived from code (the default, see [`pump`]),
//!   2. extended by `sniper.pump_extra_accounts` in the config, and
//!   3. **learned** from a real, confirmed, successful transaction on chain.
//!
//! Learning is the important one: when Pump.fun ships another silent upgrade,
//! the operator points the bot at one working transaction and it repairs
//! itself instead of requiring a code change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use bot_core::error::BotResult;

/// A slot in the template. Either we can derive it ourselves, or we record the
/// concrete pubkey we saw on chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Slot {
    /// A named account the builder knows how to derive (e.g. `bonding_curve`).
    Named {
        name: String,
        writable: bool,
        signer: bool,
    },
    /// A literal pubkey, learned from a transaction or set in the config.
    Fixed {
        pubkey: String,
        writable: bool,
        signer: bool,
    },
}

impl Slot {
    pub fn name(&self) -> &str {
        match self {
            Slot::Named { name, .. } => name,
            Slot::Fixed { pubkey, .. } => pubkey,
        }
    }

    pub fn writable(&self) -> bool {
        match self {
            Slot::Named { writable, .. } | Slot::Fixed { writable, .. } => *writable,
        }
    }

    pub fn signer(&self) -> bool {
        match self {
            Slot::Named { signer, .. } | Slot::Fixed { signer, .. } => *signer,
        }
    }

    /// Resolve this slot to an `AccountMeta` using the derived-account map.
    /// Returns `None` when a `Fixed` slot holds an unparsable pubkey.
    pub fn resolve(&self, derived: &HashMap<String, Pubkey>) -> Option<AccountMeta> {
        match self {
            Slot::Named {
                name,
                writable,
                signer,
            } => {
                let key = *derived.get(name)?;
                Some(match (writable, signer) {
                    (true, true) => AccountMeta::new(key, true),
                    (true, false) => AccountMeta::new(key, false),
                    (false, true) => AccountMeta::new_readonly(key, true),
                    (false, false) => AccountMeta::new_readonly(key, false),
                })
            }
            Slot::Fixed {
                pubkey,
                writable,
                signer,
            } => {
                let key = Pubkey::try_from(pubkey.as_str()).ok()?;
                Some(match (writable, signer) {
                    (true, true) => AccountMeta::new(key, true),
                    (true, false) => AccountMeta::new(key, false),
                    (false, true) => AccountMeta::new_readonly(key, true),
                    (false, false) => AccountMeta::new_readonly(key, false),
                })
            }
        }
    }
}

/// The ordered account list for one instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountLayout {
    /// Program the instruction targets.
    pub program_id: String,
    /// Instruction name, e.g. `buy`, `sell`, `buy_exact_sol_in`.
    pub instruction: String,
    pub slots: Vec<Slot>,
    /// Signature this template was learned from, if any.
    pub learned_from: Option<String>,
    pub learned_at: Option<String>,
    /// True when a human confirmed this layout works.
    pub trusted: bool,
}

impl AccountLayout {
    pub fn new(program_id: Pubkey, instruction: impl Into<String>, slots: Vec<Slot>) -> Self {
        AccountLayout {
            program_id: program_id.to_string(),
            instruction: instruction.into(),
            slots,
            learned_from: None,
            learned_at: None,
            trusted: false,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Resolve every slot. Any slot that cannot be resolved is reported so the
    /// caller can log a precise diagnostic instead of failing mysteriously.
    pub fn build(&self, derived: &HashMap<String, Pubkey>) -> BotResult<Vec<AccountMeta>> {
        let mut out = Vec::with_capacity(self.slots.len());
        let mut missing = Vec::new();
        for slot in &self.slots {
            match slot.resolve(derived) {
                Some(meta) => out.push(meta),
                None => missing.push(slot.name().to_string()),
            }
        }
        if !missing.is_empty() {
            return Err(bot_core::BotError::solana(format!(
                "cannot resolve account layout slots for {}: {missing:?}",
                self.instruction
            )));
        }
        Ok(out)
    }

    /// Append extra literal accounts (from config) in order.
    pub fn push_fixed(&mut self, pubkey: Pubkey, writable: bool, signer: bool) {
        self.slots.push(Slot::Fixed {
            pubkey: pubkey.to_string(),
            writable,
            signer,
        });
    }

    /// Append a derived (named) account.
    pub fn push_named(&mut self, name: impl Into<String>, writable: bool, signer: bool) {
        self.slots.push(Slot::Named {
            name: name.into(),
            writable,
            signer,
        });
    }

    /// Replace this template with one learned from a confirmed transaction.
    ///
    /// `accounts` is the resolved account list of the instruction, in order,
    /// with the flags the transaction actually used. Slots whose pubkey we can
    /// name (because it matches something in `known_names`) stay `Named` so the
    /// template keeps working for other mints; everything else becomes `Fixed`.
    pub fn learn(
        &mut self,
        accounts: &[AccountMeta],
        known_names: &HashMap<Pubkey, String>,
        signature: Option<&str>,
    ) {
        let mut slots = Vec::with_capacity(accounts.len());
        for meta in accounts {
            match known_names.get(&meta.pubkey) {
                Some(name) => slots.push(Slot::Named {
                    name: name.clone(),
                    writable: meta.is_writable,
                    signer: meta.is_signer,
                }),
                None => slots.push(Slot::Fixed {
                    pubkey: meta.pubkey.to_string(),
                    writable: meta.is_writable,
                    signer: meta.is_signer,
                }),
            }
        }
        let changed = slots.len() != self.slots.len();
        self.slots = slots;
        self.learned_from = signature.map(|s| s.to_string());
        self.learned_at = Some(chrono::Utc::now().to_rfc3339());
        self.trusted = true;
        if changed {
            info!(
                instruction = %self.instruction,
                accounts = self.slots.len(),
                signature = self.learned_from.clone().unwrap_or_default(),
                "learned a new account layout from a confirmed transaction"
            );
        }
    }
}

/// On-disk store of learned layouts, keyed by `program:instruction`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutStore {
    pub layouts: HashMap<String, AccountLayout>,
}

impl LayoutStore {
    pub fn key(program_id: &Pubkey, instruction: &str) -> String {
        format!("{program_id}:{instruction}")
    }

    pub fn get(&self, program_id: &Pubkey, instruction: &str) -> Option<&AccountLayout> {
        self.layouts.get(&Self::key(program_id, instruction))
    }

    pub fn insert(&mut self, layout: AccountLayout) {
        let key = Self::key(
            &Pubkey::try_from(layout.program_id.as_str()).unwrap_or_default(),
            &layout.instruction,
        );
        self.layouts.insert(key, layout);
    }

    /// Load from disk. A missing or corrupt file is not fatal: we just start
    /// from the code-derived defaults, which is the correct fallback.
    pub async fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        if !path.exists() {
            return LayoutStore::default();
        }
        match tokio::fs::read_to_string(path).await {
            Ok(text) => match serde_json::from_str::<LayoutStore>(&text) {
                Ok(store) => {
                    info!(
                        path = %path.display(),
                        layouts = store.layouts.len(),
                        "loaded learned account layouts"
                    );
                    store
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "ignoring corrupt layout file");
                    LayoutStore::default()
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "cannot read layout file");
                LayoutStore::default()
            }
        }
    }

    /// Persist to disk, creating the parent directory if needed.
    pub async fn save(&self, path: impl AsRef<Path>) -> BotResult<()> {
        let path: PathBuf = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let text = serde_json::to_string_pretty(self)?;
        // Write to a temp file then rename, so a crash cannot truncate the store.
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, text).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn derived() -> HashMap<String, Pubkey> {
        let mut m = HashMap::new();
        m.insert("mint".to_string(), Pubkey::new_unique());
        m.insert("user".to_string(), Pubkey::new_unique());
        m
    }

    #[test]
    fn build_reports_missing_slots_precisely() {
        let mut layout = AccountLayout::new(Pubkey::new_unique(), "buy", Vec::new());
        layout.push_named("mint", false, false);
        layout.push_named("does_not_exist", true, false);
        let err = layout.build(&derived()).unwrap_err();
        assert!(err.to_string().contains("does_not_exist"), "{err}");
    }

    #[test]
    fn flags_survive_resolution() {
        let mut layout = AccountLayout::new(Pubkey::new_unique(), "buy", Vec::new());
        layout.push_named("user", true, true);
        layout.push_named("mint", false, false);
        let metas = layout.build(&derived()).unwrap();
        assert!(metas[0].is_signer && metas[0].is_writable);
        assert!(!metas[1].is_signer && !metas[1].is_writable);
    }

    #[test]
    fn learn_keeps_named_slots_and_pins_unknown_ones() {
        let d = derived();
        let mint = d["mint"];
        let mystery = Pubkey::new_unique();
        let accounts = vec![
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new(mystery, false),
        ];
        let mut known = HashMap::new();
        known.insert(mint, "mint".to_string());

        let mut layout = AccountLayout::new(Pubkey::new_unique(), "buy", Vec::new());
        layout.learn(&accounts, &known, Some("5xyZ..."));

        assert_eq!(layout.len(), 2);
        assert!(layout.trusted);
        assert_eq!(layout.learned_from.as_deref(), Some("5xyZ..."));
        assert!(matches!(&layout.slots[0], Slot::Named { name, .. } if name == "mint"));
        assert!(
            matches!(&layout.slots[1], Slot::Fixed { pubkey, .. } if pubkey == &mystery.to_string())
        );

        // The learned template must still resolve without any extra inputs.
        let metas = layout.build(&derived()).unwrap();
        assert_eq!(metas[1].pubkey, mystery);
    }

    #[tokio::test]
    async fn save_and_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("layout-test-{}", std::process::id()));
        let path = dir.join("layout.json");
        let mut store = LayoutStore::default();
        let mut layout = AccountLayout::new(Pubkey::new_unique(), "buy", Vec::new());
        layout.push_named("mint", false, false);
        store.insert(layout.clone());
        store.save(&path).await.unwrap();

        let loaded = LayoutStore::load(&path).await;
        assert_eq!(loaded.layouts.len(), 1);
        let got = loaded
            .get(
                &Pubkey::try_from(layout.program_id.as_str()).unwrap(),
                "buy",
            )
            .expect("layout must survive the round trip");
        assert_eq!(got.len(), 1);

        // A corrupt file must degrade to empty, never panic.
        tokio::fs::write(&path, "{ not json").await.unwrap();
        let broken = LayoutStore::load(&path).await;
        assert!(broken.layouts.is_empty());

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
