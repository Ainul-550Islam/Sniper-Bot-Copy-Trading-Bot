//! Instruction definitions and (de)serialisation.
//!
//! Instructions are borsh-encoded; the first byte is the variant discriminant.
//! Each variant documents the accounts it expects (see `processor` for the
//! authoritative ordering and signer/writable flags).

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    sysvar,
};
use solana_system_interface::program as system_program;

use crate::error::StakingError;
use crate::state::{
    config_pda, metadata_pda, stake_pda, METADATA_NAME_MAX_LEN, METADATA_SYMBOL_MAX_LEN,
    METADATA_URI_MAX_LEN,
};

/// The program's instruction set.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum StakingInstruction {
    /// Create the mint, vault and treasury token accounts and store the config.
    ///
    /// `max_supply` is the ABSOLUTE total-supply cap for the mint (raw token
    /// units, must be > 0). It is immutable after this call: `UpdateParams`
    /// cannot change it, so no admin action can ever raise the cap. Genesis
    /// minting and reward minting are both enforced against it.
    ///
    /// Accounts:
    /// 0. `[writable, signer]` Payer / admin.
    /// 1. `[writable]` Config PDA.
    /// 2. `[writable, signer]` Mint (new keypair).
    /// 3. `[writable]` Vault (ATA of config PDA).
    /// 4. `[writable]` Treasury token account (ATA of treasury wallet).
    /// 5. `[]` Treasury wallet.
    /// 6. `[]` Token program.
    /// 7. `[]` Associated token program.
    /// 8. `[]` System program.
    /// 9. `[]` Rent sysvar.
    Initialize {
        fee_bps: u16,
        reward_rate_bps: u64,
        min_stake: u64,
        unstake_delay: i64,
        decimals: u8,
        timelock_secs: i64,
        max_supply: u64,
    },
    /// Deposit `amount` tokens (fee goes to the treasury).
    ///
    /// Accounts:
    /// 0. `[writable, signer]` Staker.
    /// 1. `[writable]` Staker's source token account.
    /// 2. `[writable]` Vault.
    /// 3. `[writable]` Treasury token account.
    /// 4. `[writable]` Stake PDA.
    /// 5. `[]` Config PDA.
    /// 6. `[]` Token program.
    /// 7. `[]` System program.
    /// 8. `[]` Clock sysvar.
    Stake { amount: u64 },
    /// Withdraw principal + accrued rewards after the cooldown.
    ///
    /// Accounts:
    /// 0. `[writable, signer]` Staker.
    /// 1. `[writable]` Staker's destination token account.
    /// 2. `[writable]` Vault.
    /// 3. `[writable]` Mint.
    /// 4. `[writable]` Stake PDA.
    /// 5. `[]` Config PDA.
    /// 6. `[]` Token program.
    /// 7. `[]` Clock sysvar.
    Unstake,
    /// Claim accrued rewards without withdrawing principal.
    ///
    /// Accounts: same as `Unstake`.
    Claim,
    /// Admin: queue a parameter update. It only takes effect after the timelock
    /// delay via `ApplyParams` (which anyone may call once the delay elapses).
    /// `None` fields keep their current values. Accounts:
    /// 0. `[signer]` Admin. 1. `[writable]` Config PDA. 2. `[]` Clock sysvar.
    UpdateParams {
        fee_bps: Option<u16>,
        reward_rate_bps: Option<u64>,
        min_stake: Option<u64>,
        unstake_delay: Option<i64>,
        timelock_secs: Option<i64>,
    },
    /// Apply the queued parameter update once the timelock has elapsed.
    /// Permissionless. Accounts:
    /// 0. `[writable]` Config PDA. 1. `[]` Clock sysvar.
    ApplyParams,
    /// Admin: cancel the queued parameter update. Accounts:
    /// 0. `[signer]` Admin. 1. `[writable]` Config PDA.
    CancelParams,
    /// Admin: halt new deposits (withdrawals stay enabled). Accounts:
    /// 0. `[signer]` Admin. 1. `[writable]` Config PDA.
    Pause,
    /// Admin: resume deposits. Accounts: same as `Pause`.
    Unpause,
    /// Admin: propose a new admin (step 1 of a two-step transfer). Accounts:
    /// 0. `[signer]` Admin. 1. `[writable]` Config PDA.
    TransferAdmin { new_admin: Pubkey },
    /// Proposed admin: accept the transfer, becoming the admin (step 2).
    /// Accounts: 0. `[signer]` Pending admin. 1. `[writable]` Config PDA.
    AcceptAdmin,
    /// Admin: ONE-TIME genesis mint of the initial supply to a recipient token
    /// account. Succeeds at most once per deployment (latched by
    /// `Config::genesis_done`); every later attempt fails with
    /// `GenesisAlreadyDone`. This is the only sanctioned initial-distribution
    /// path — after it, minting is limited to reward accrual on stake.
    /// Bounded by `Config::max_supply`: fails with `MaxSupplyExceeded` unless
    /// the live mint supply plus `amount` stays at or below the cap.
    ///
    /// Accounts:
    /// 0. `[signer]` Admin.
    /// 1. `[writable]` Config PDA.
    /// 2. `[writable]` Mint (must equal `Config::mint`).
    /// 3. `[writable]` Recipient token account (of the mint).
    /// 4. `[]` Token program.
    GenesisMint { amount: u64 },
    /// Admin: create the SPL token-metadata account for the program mint via
    /// a CPI to mpl-token-metadata (`CreateMetadataAccountsV3`). ONE-SHOT:
    /// once the metadata PDA exists, every later call fails with
    /// `MetadataAlreadyExists`. The metadata is created IMMUTABLE
    /// (`is_mutable = false`) with the config PDA as both mint authority and
    /// update authority, so neither the admin nor anyone else can rewrite
    /// name/symbol/uri afterwards. Field lengths are validated against the
    /// mpl limits (name <= 32, symbol <= 10, uri <= 200, none empty) before
    /// the CPI.
    ///
    /// Accounts:
    /// 0. `[writable, signer]` Admin (pays the metadata account rent).
    /// 1. `[]` Config PDA (mint + update authority; signs the CPI via PDA).
    /// 2. `[]` Mint (must equal `Config::mint`).
    /// 3. `[writable]` Metadata PDA (`["metadata", metadata_program, mint]`
    ///    under the metadata program).
    /// 4. `[]` Token Metadata program (`metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s`).
    /// 5. `[]` System program.
    /// 6. `[]` Rent sysvar.
    CreateTokenMetadata {
        name: String,
        symbol: String,
        uri: String,
    },
}

impl StakingInstruction {
    /// Borsh-encode to instruction data.
    pub fn pack(&self) -> Result<Vec<u8>, StakingError> {
        borsh::to_vec(self).map_err(|_| StakingError::InvalidInstructionData)
    }

    /// Decode instruction data.
    pub fn unpack(data: &[u8]) -> Result<Self, StakingError> {
        Self::try_from_slice(data).map_err(|_| StakingError::InvalidInstructionData)
    }
}

// ---------------------------------------------------------------------------
// Client-side instruction builders (used by the CLI and tests).
// ---------------------------------------------------------------------------

/// Build a `Stake` instruction.
pub fn stake_ix(
    program_id: &Pubkey,
    staker: &Pubkey,
    staker_token: &Pubkey,
    vault: &Pubkey,
    treasury: &Pubkey,
    amount: u64,
) -> Result<Instruction, StakingError> {
    let (config_pda_key, _) = config_pda(program_id);
    let (stake_key, _) = stake_pda(program_id, staker);
    let data = StakingInstruction::Stake { amount }.pack()?;
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*staker, true),
            AccountMeta::new(*staker_token, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*treasury, false),
            AccountMeta::new(stake_key, false),
            AccountMeta::new_readonly(config_pda_key, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data,
    })
}

/// Build an `Unstake` instruction.
pub fn unstake_ix(
    program_id: &Pubkey,
    staker: &Pubkey,
    staker_token: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
) -> Result<Instruction, StakingError> {
    let (config_pda_key, _) = config_pda(program_id);
    let (stake_key, _) = stake_pda(program_id, staker);
    let data = StakingInstruction::Unstake.pack()?;
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*staker, true),
            AccountMeta::new(*staker_token, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*mint, false),
            AccountMeta::new(stake_key, false),
            AccountMeta::new_readonly(config_pda_key, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data,
    })
}

/// Build a `Claim` instruction (same accounts as unstake).
pub fn claim_ix(
    program_id: &Pubkey,
    staker: &Pubkey,
    staker_token: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
) -> Result<Instruction, StakingError> {
    let mut ix = unstake_ix(program_id, staker, staker_token, vault, mint)?;
    ix.data = StakingInstruction::Claim.pack()?;
    Ok(ix)
}

/// Build an admin-only instruction (`Pause` / `Unpause` / `TransferAdmin` /
/// `AcceptAdmin` / `CancelParams`). They all take the same two accounts:
/// 0. `[signer]` the authority (current admin, or pending admin for accept).
/// 1. `[writable]` the config PDA.
pub fn admin_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    ix: StakingInstruction,
) -> Result<Instruction, StakingError> {
    let (config_pda_key, _) = config_pda(program_id);
    let data = ix.pack()?;
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*authority, true),
            AccountMeta::new(config_pda_key, false),
        ],
        data,
    })
}

/// Build an `UpdateParams` (queue) instruction. Accounts:
/// 0. `[signer]` admin. 1. `[writable]` config PDA. 2. `[]` clock sysvar.
#[allow(clippy::too_many_arguments)]
pub fn update_params_ix(
    program_id: &Pubkey,
    admin: &Pubkey,
    fee_bps: Option<u16>,
    reward_rate_bps: Option<u64>,
    min_stake: Option<u64>,
    unstake_delay: Option<i64>,
    timelock_secs: Option<i64>,
) -> Result<Instruction, StakingError> {
    let (config_pda_key, _) = config_pda(program_id);
    let data = StakingInstruction::UpdateParams {
        fee_bps,
        reward_rate_bps,
        min_stake,
        unstake_delay,
        timelock_secs,
    }
    .pack()?;
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*admin, true),
            AccountMeta::new(config_pda_key, false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data,
    })
}

/// Build an `ApplyParams` instruction (permissionless once the timelock has
/// elapsed). Accounts: 0. `[writable]` config PDA. 1. `[]` clock sysvar.
pub fn apply_params_ix(program_id: &Pubkey) -> Result<Instruction, StakingError> {
    let (config_pda_key, _) = config_pda(program_id);
    let data = StakingInstruction::ApplyParams.pack()?;
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(config_pda_key, false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data,
    })
}

/// Build a `GenesisMint` instruction (one-time initial distribution).
pub fn genesis_mint_ix(
    program_id: &Pubkey,
    admin: &Pubkey,
    mint: &Pubkey,
    recipient: &Pubkey,
    amount: u64,
) -> Result<Instruction, StakingError> {
    let (config_key, _) = config_pda(program_id);
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*admin, true),
            AccountMeta::new(config_key, false),
            AccountMeta::new(*mint, false),
            AccountMeta::new(*recipient, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data: StakingInstruction::GenesisMint { amount }.pack()?,
    })
}

/// Build a `CreateTokenMetadata` instruction. Validates the field lengths up
/// front (same limits the processor enforces) so a doomed transaction is
/// never assembled. Accounts (processor order):
/// 0. `[writable, signer]` admin/payer. 1. `[]` config PDA. 2. `[]` mint.
/// 3. `[writable]` metadata PDA. 4. `[]` metadata program. 5. `[]` system
/// program. 6. `[]` rent sysvar.
pub fn create_token_metadata_ix(
    program_id: &Pubkey,
    admin: &Pubkey,
    mint: &Pubkey,
    name: &str,
    symbol: &str,
    uri: &str,
) -> Result<Instruction, StakingError> {
    validate_metadata_fields(name, symbol, uri)?;
    let (config_key, _) = config_pda(program_id);
    let (metadata_key, _) = metadata_pda(mint);
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*admin, true),
            AccountMeta::new_readonly(config_key, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(metadata_key, false),
            AccountMeta::new_readonly(crate::state::TOKEN_METADATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
        ],
        data: StakingInstruction::CreateTokenMetadata {
            name: name.to_string(),
            symbol: symbol.to_string(),
            uri: uri.to_string(),
        }
        .pack()?,
    })
}

/// Shared field validation for token metadata (builder + processor): none of
/// name/symbol/uri may be empty and each must fit the mpl-token-metadata
/// byte-length limits (32 / 10 / 200 bytes). Byte length is checked (not
/// char count) because mpl measures bytes — this is the stricter bound, so a
/// transaction passing here can never fail the CPI's own length checks.
pub fn validate_metadata_fields(name: &str, symbol: &str, uri: &str) -> Result<(), StakingError> {
    if name.is_empty() || name.len() > METADATA_NAME_MAX_LEN {
        return Err(StakingError::MetadataFieldTooLong);
    }
    if symbol.is_empty() || symbol.len() > METADATA_SYMBOL_MAX_LEN {
        return Err(StakingError::MetadataFieldTooLong);
    }
    if uri.is_empty() || uri.len() > METADATA_URI_MAX_LEN {
        return Err(StakingError::MetadataFieldTooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_roundtrips() {
        let ix = StakingInstruction::Initialize {
            fee_bps: 100,
            reward_rate_bps: 1000,
            min_stake: 1_000_000,
            unstake_delay: 3600,
            decimals: 6,
            timelock_secs: 86_400,
            max_supply: 1_000_000_000_000,
        };
        let bytes = ix.pack().unwrap();
        assert_eq!(StakingInstruction::unpack(&bytes).unwrap(), ix);
    }

    #[test]
    fn stake_instruction_roundtrips() {
        let ix = StakingInstruction::Stake { amount: 42 };
        let bytes = ix.pack().unwrap();
        assert_eq!(StakingInstruction::unpack(&bytes).unwrap(), ix);
        // Discriminant differs between variants.
        let other = StakingInstruction::Unstake.pack().unwrap();
        assert_ne!(bytes[0], other[0]);
    }

    #[test]
    fn genesis_mint_roundtrips_with_distinct_discriminant() {
        let ix = StakingInstruction::GenesisMint { amount: 123_456 };
        let bytes = ix.pack().unwrap();
        assert_eq!(StakingInstruction::unpack(&bytes).unwrap(), ix);
        assert_ne!(bytes[0], StakingInstruction::AcceptAdmin.pack().unwrap()[0]);
    }

    #[test]
    fn update_params_with_options_roundtrips() {
        let ix = StakingInstruction::UpdateParams {
            fee_bps: Some(50),
            reward_rate_bps: None,
            min_stake: Some(5),
            unstake_delay: None,
            timelock_secs: Some(7_200),
        };
        let bytes = ix.pack().unwrap();
        assert_eq!(StakingInstruction::unpack(&bytes).unwrap(), ix);
    }

    #[test]
    fn governance_variants_roundtrip_with_distinct_discriminants() {
        let variants = [
            StakingInstruction::Pause,
            StakingInstruction::Unpause,
            StakingInstruction::TransferAdmin {
                new_admin: Pubkey::new_unique(),
            },
            StakingInstruction::AcceptAdmin,
            StakingInstruction::ApplyParams,
            StakingInstruction::CancelParams,
        ];
        let mut discriminants = std::collections::HashSet::new();
        for v in &variants {
            let bytes = v.pack().unwrap();
            assert_eq!(&StakingInstruction::unpack(&bytes).unwrap(), v);
            discriminants.insert(bytes[0]);
        }
        assert_eq!(
            discriminants.len(),
            variants.len(),
            "every variant must have a distinct discriminant"
        );
    }

    #[test]
    fn governance_builders_pin_the_config_pda() {
        let pid = Pubkey::new_unique();
        let admin = Pubkey::new_unique();
        let (config_key, _) = config_pda(&pid);

        let up = update_params_ix(&pid, &admin, Some(10), None, None, None, None).unwrap();
        assert_eq!(up.accounts[0].pubkey, admin);
        assert!(up.accounts[0].is_signer);
        assert_eq!(up.accounts[1].pubkey, config_key);
        assert!(up.accounts[1].is_writable);
        assert_eq!(up.accounts[2].pubkey, sysvar::clock::id());

        let ap = apply_params_ix(&pid).unwrap();
        assert_eq!(ap.accounts[0].pubkey, config_key);
        assert!(ap.accounts[0].is_writable);
        assert!(
            !ap.accounts.iter().any(|a| a.is_signer),
            "apply is permissionless"
        );

        let cx = admin_ix(&pid, &admin, StakingInstruction::CancelParams).unwrap();
        assert_eq!(cx.accounts.len(), 2);
        assert!(cx.accounts[0].is_signer);
    }

    #[test]
    fn create_token_metadata_roundtrips() {
        let ix = StakingInstruction::CreateTokenMetadata {
            name: "Sniper Suite Token".into(),
            symbol: "SNPR".into(),
            uri: "https://example.com/snpr.json".into(),
        };
        let bytes = ix.pack().unwrap();
        assert_eq!(StakingInstruction::unpack(&bytes).unwrap(), ix);
        // Distinct discriminant from every other variant already tested.
        assert_ne!(
            bytes[0],
            StakingInstruction::GenesisMint { amount: 1 }
                .pack()
                .unwrap()[0]
        );
        assert_ne!(bytes[0], StakingInstruction::AcceptAdmin.pack().unwrap()[0]);
    }

    #[test]
    fn metadata_field_validation_enforces_mpl_limits() {
        // Valid baseline.
        assert!(validate_metadata_fields("Name", "SYM", "https://x/y.json").is_ok());
        // Exact-limit values are allowed (boundary).
        assert!(validate_metadata_fields(
            &"n".repeat(METADATA_NAME_MAX_LEN),
            &"s".repeat(METADATA_SYMBOL_MAX_LEN),
            &"u".repeat(METADATA_URI_MAX_LEN),
        )
        .is_ok());
        // One over each limit is rejected.
        assert_eq!(
            validate_metadata_fields(&"n".repeat(METADATA_NAME_MAX_LEN + 1), "S", "u"),
            Err(StakingError::MetadataFieldTooLong)
        );
        assert_eq!(
            validate_metadata_fields("N", &"s".repeat(METADATA_SYMBOL_MAX_LEN + 1), "u"),
            Err(StakingError::MetadataFieldTooLong)
        );
        assert_eq!(
            validate_metadata_fields("N", "S", &"u".repeat(METADATA_URI_MAX_LEN + 1)),
            Err(StakingError::MetadataFieldTooLong)
        );
        // Multi-byte characters count as BYTES (mpl's own measure): 11 x "é"
        // is 22 bytes > the 10-byte symbol limit even though it is 11 chars.
        assert_eq!(
            validate_metadata_fields("N", &"é".repeat(11), "u"),
            Err(StakingError::MetadataFieldTooLong)
        );
        // Empty fields are rejected.
        assert_eq!(
            validate_metadata_fields("", "S", "u"),
            Err(StakingError::MetadataFieldTooLong)
        );
        assert_eq!(
            validate_metadata_fields("N", "", "u"),
            Err(StakingError::MetadataFieldTooLong)
        );
        assert_eq!(
            validate_metadata_fields("N", "S", ""),
            Err(StakingError::MetadataFieldTooLong)
        );
    }

    #[test]
    fn metadata_builder_pins_pda_and_programs() {
        let pid = crate::id();
        let admin = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ix = create_token_metadata_ix(&pid, &admin, &mint, "Name", "SYM", "uri")
            .expect("valid fields build");
        assert_eq!(ix.program_id, pid);
        let (config_key, _) = config_pda(&pid);
        let (metadata_key, _) = metadata_pda(&mint);
        assert_eq!(ix.accounts.len(), 7);
        assert_eq!(ix.accounts[0].pubkey, admin);
        assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, config_key);
        assert_eq!(ix.accounts[2].pubkey, mint);
        assert_eq!(ix.accounts[3].pubkey, metadata_key);
        assert!(ix.accounts[3].is_writable);
        assert_eq!(
            ix.accounts[4].pubkey,
            crate::state::TOKEN_METADATA_PROGRAM_ID
        );
        assert_eq!(ix.accounts[5].pubkey, system_program::id());
        assert_eq!(ix.accounts[6].pubkey, sysvar::rent::id());
        // Bad fields are rejected by the builder itself.
        assert_eq!(
            create_token_metadata_ix(&pid, &admin, &mint, "", "SYM", "uri"),
            Err(StakingError::MetadataFieldTooLong)
        );
    }

    #[test]
    fn bad_data_is_rejected() {
        assert!(StakingInstruction::unpack(&[9, 9, 9, 9, 9]).is_err());
    }

    #[test]
    fn client_builder_sets_the_program_and_signer() {
        let pid = Pubkey::new_unique();
        let staker = Pubkey::new_unique();
        let ix = stake_ix(
            &pid,
            &staker,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            1000,
        )
        .unwrap();
        assert_eq!(ix.program_id, pid);
        assert!(ix.accounts[0].is_signer);
        assert!(ix.accounts[0].is_writable);
        // Config PDA is present and read-only.
        assert!(ix
            .accounts
            .iter()
            .any(|a| a.pubkey == config_pda(&pid).0 && !a.is_writable));
    }
}
