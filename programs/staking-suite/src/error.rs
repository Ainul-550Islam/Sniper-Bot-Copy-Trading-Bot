//! Program errors.
//!
//! Custom variants are mapped into `ProgramError::Custom` starting at 6000 so
//! they are easy to spot in transaction logs and never collide with the SPL
//! token program's own error space.

use solana_program::program_error::ProgramError;
use thiserror::Error;

/// First custom error code.
pub const CUSTOM_ERROR_BASE: u32 = 6000;

/// Errors returned by the staking program.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum StakingError {
    #[error("Config account is already initialized")]
    AlreadyInitialized,
    #[error("Instruction data did not deserialize")]
    InvalidInstructionData,
    #[error("Signer is not the admin")]
    Unauthorized,
    #[error("Amount is below the minimum stake")]
    BelowMinimum,
    #[error("Unstake cooldown has not elapsed")]
    CooldownActive,
    #[error("No stake to withdraw")]
    InsufficientStake,
    #[error("Numeric overflow")]
    Overflow,
    #[error("A provided account is invalid")]
    InvalidAccount,
    #[error("Arithmetic error")]
    Arithmetic,
    #[error("Token program account is not the SPL Token program")]
    InvalidTokenProgram,
    #[error("System program account is not the System program")]
    InvalidSystemProgram,
    #[error("Associated token program account is not the ATA program")]
    InvalidAssociatedTokenProgram,
    #[error("Config account is not the program's initialised config PDA")]
    InvalidConfigAccount,
    #[error("Stake account is not the staker's program-owned PDA")]
    InvalidStakeAccount,
    #[error("Vault account does not match the config vault")]
    InvalidVault,
    #[error("Mint account does not match the config mint")]
    InvalidMint,
    #[error("Treasury account does not match the config treasury")]
    InvalidTreasury,
    #[error("Staker token account is not owned by the staker or has the wrong mint")]
    InvalidStakerToken,
    #[error("Deposits are paused")]
    Paused,
    #[error("Deposit fee exceeds the hard cap")]
    FeeTooHigh,
    #[error("Reward rate exceeds the hard cap")]
    RewardRateTooHigh,
    #[error("Signer is not the pending admin")]
    NotPendingAdmin,
    #[error("A parameter update is already queued")]
    UpdateAlreadyQueued,
    #[error("No parameter update is queued")]
    NoPendingUpdate,
    #[error("The parameter timelock has not elapsed")]
    TimelockNotElapsed,
    #[error("Timelock delay is out of range")]
    TimelockOutOfRange,
    #[error("Genesis mint has already been performed for this deployment")]
    GenesisAlreadyDone,
    #[error("Amount must be greater than zero")]
    InvalidAmount,
    #[error("Mint would exceed the immutable max supply")]
    MaxSupplyExceeded,
    #[error("max_supply must be greater than zero")]
    InvalidMaxSupply,
    #[error("Token metadata account already exists (one-shot instruction)")]
    MetadataAlreadyExists,
    #[error("Token metadata program account is not mpl-token-metadata")]
    InvalidMetadataProgram,
    #[error("Metadata name/symbol/uri empty or longer than the mpl limit")]
    MetadataFieldTooLong,
}

impl From<StakingError> for ProgramError {
    fn from(e: StakingError) -> Self {
        let code = match e {
            StakingError::AlreadyInitialized => 0,
            StakingError::InvalidInstructionData => 1,
            StakingError::Unauthorized => 2,
            StakingError::BelowMinimum => 3,
            StakingError::CooldownActive => 4,
            StakingError::InsufficientStake => 5,
            StakingError::Overflow => 6,
            StakingError::InvalidAccount => 7,
            StakingError::Arithmetic => 8,
            StakingError::InvalidTokenProgram => 9,
            StakingError::InvalidSystemProgram => 10,
            StakingError::InvalidAssociatedTokenProgram => 11,
            StakingError::InvalidConfigAccount => 12,
            StakingError::InvalidStakeAccount => 13,
            StakingError::InvalidVault => 14,
            StakingError::InvalidMint => 15,
            StakingError::InvalidTreasury => 16,
            StakingError::InvalidStakerToken => 17,
            StakingError::Paused => 18,
            StakingError::FeeTooHigh => 19,
            StakingError::RewardRateTooHigh => 20,
            StakingError::NotPendingAdmin => 21,
            StakingError::UpdateAlreadyQueued => 22,
            StakingError::NoPendingUpdate => 23,
            StakingError::TimelockNotElapsed => 24,
            StakingError::TimelockOutOfRange => 25,
            StakingError::GenesisAlreadyDone => 26,
            StakingError::InvalidAmount => 27,
            StakingError::MaxSupplyExceeded => 28,
            StakingError::InvalidMaxSupply => 29,
            StakingError::MetadataAlreadyExists => 30,
            StakingError::InvalidMetadataProgram => 31,
            StakingError::MetadataFieldTooLong => 32,
        };
        ProgramError::Custom(CUSTOM_ERROR_BASE + code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_into_the_custom_range() {
        let pe: ProgramError = StakingError::Unauthorized.into();
        match pe {
            ProgramError::Custom(c) => assert_eq!(c, CUSTOM_ERROR_BASE + 2),
            _ => panic!("expected a custom error"),
        }
    }

    #[test]
    fn distinct_errors_have_distinct_codes() {
        let a: ProgramError = StakingError::BelowMinimum.into();
        let b: ProgramError = StakingError::CooldownActive.into();
        assert_ne!(a, b);
    }

    #[test]
    fn every_variant_has_a_unique_code() {
        use StakingError::*;
        let all = [
            AlreadyInitialized,
            InvalidInstructionData,
            Unauthorized,
            BelowMinimum,
            CooldownActive,
            InsufficientStake,
            Overflow,
            InvalidAccount,
            Arithmetic,
            InvalidTokenProgram,
            InvalidSystemProgram,
            InvalidAssociatedTokenProgram,
            InvalidConfigAccount,
            InvalidStakeAccount,
            InvalidVault,
            InvalidMint,
            InvalidTreasury,
            InvalidStakerToken,
            Paused,
            FeeTooHigh,
            RewardRateTooHigh,
            NotPendingAdmin,
            UpdateAlreadyQueued,
            NoPendingUpdate,
            TimelockNotElapsed,
            TimelockOutOfRange,
            GenesisAlreadyDone,
            InvalidAmount,
            MaxSupplyExceeded,
            InvalidMaxSupply,
            MetadataAlreadyExists,
            InvalidMetadataProgram,
            MetadataFieldTooLong,
        ];
        let mut codes: Vec<u32> = all
            .iter()
            .map(|e| match ProgramError::from(e.clone()) {
                ProgramError::Custom(c) => c,
                _ => panic!("expected a custom error"),
            })
            .collect();
        let n = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), n, "two errors share a custom code");
        // All codes live in the documented custom range.
        assert!(codes.iter().all(|c| *c >= CUSTOM_ERROR_BASE));
    }
}
