//! Meteora venue adapters (GAP-MAP v2, P2): DLMM and DAMM v1.
//!
//! Verification status at pin time (2026-10-08, sources in
//! docs/COMPETITOR-BENCHMARK.md §1 and this file's history):
//! * **DLMM** `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` — confirmed by
//!   multiple independent 2026 sources; highest-volume Solana DEX family.
//! * **DAMM v1** `Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB` — single
//!   source at pin time; re-verify before production use.
//!
//! As with every adapter in `venues/`, instruction/account layouts are NOT
//! yet verified, so swap building refuses (`LayoutUnverified`). DLMM in
//! particular needs care: its liquidity lives in BIN ARRAYS, the swap
//! account set depends on which bin arrays straddle the active bin, and the
//! program also routes through an event authority — none of which may be
//! guessed. The honest state is "identity known, layout pending".

use solana_sdk::pubkey::Pubkey;

use crate::consts::{METEORA_DAMM_V1, METEORA_DLMM};
use crate::venues::VenueAdapter;

/// Meteora DLMM adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeteoraDlmm {
    enabled: bool,
}

impl MeteoraDlmm {
    pub fn new() -> Self {
        MeteoraDlmm { enabled: true }
    }

    pub fn with_enabled(enabled: bool) -> Self {
        MeteoraDlmm { enabled }
    }

    /// Layout-free ownership check (see raydium_cpmm for the pattern).
    pub fn is_pair_owner(&self, account_owner: &Pubkey) -> bool {
        account_owner == &*METEORA_DLMM
    }
}

impl VenueAdapter for MeteoraDlmm {
    fn id(&self) -> &'static str {
        "meteora_dlmm"
    }

    fn label(&self) -> &'static str {
        "Meteora DLMM"
    }

    fn program_id(&self) -> &Pubkey {
        &METEORA_DLMM
    }

    fn layout_verified(&self) -> bool {
        // Requires: DLMM IDL review, LbPair/BinArray decoding with fixture
        // tests, and a devnet swap simulation recorded in evidence/live/.
        false
    }

    fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Meteora DAMM v1 (dynamic AMM pools) adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeteoraDammV1 {
    enabled: bool,
}

impl MeteoraDammV1 {
    pub fn new() -> Self {
        MeteoraDammV1 { enabled: true }
    }

    pub fn with_enabled(enabled: bool) -> Self {
        MeteoraDammV1 { enabled }
    }

    pub fn is_pool_owner(&self, account_owner: &Pubkey) -> bool {
        account_owner == &*METEORA_DAMM_V1
    }
}

impl VenueAdapter for MeteoraDammV1 {
    fn id(&self) -> &'static str {
        "meteora_damm_v1"
    }

    fn label(&self) -> &'static str {
        "Meteora DAMM v1"
    }

    fn program_id(&self) -> &Pubkey {
        &METEORA_DAMM_V1
    }

    fn layout_verified(&self) -> bool {
        // Also re-verify the PROGRAM ID itself (single-source at pin time).
        false
    }

    fn enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_checks_match_pinned_ids() {
        let dlmm = MeteoraDlmm::new();
        assert!(dlmm.is_pair_owner(&METEORA_DLMM));
        assert!(!dlmm.is_pair_owner(&METEORA_DAMM_V1));
        let damm = MeteoraDammV1::new();
        assert!(damm.is_pool_owner(&METEORA_DAMM_V1));
        assert!(!damm.is_pool_owner(&METEORA_DLMM));
    }

    #[test]
    fn both_adapters_refuse_builds_until_verified() {
        use crate::venues::{VenueError, VenueSwapRequest};
        let req = VenueSwapRequest {
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            buy: false,
            amount_in: 42,
            min_out: 1,
        };
        for adapter in [&MeteoraDlmm::new() as &dyn VenueAdapter, &MeteoraDammV1::new()] {
            let err = adapter.build_swap(&req).unwrap_err();
            assert!(err.to_string().contains("unverified"));
            assert_eq!(
                VenueError::LayoutUnverified.into_bot_error(adapter.id()).to_string(),
                err.to_string()
            );
        }
    }
}
