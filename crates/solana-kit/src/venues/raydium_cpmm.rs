//! Raydium CPMM venue adapter (GAP-MAP v2, P2).
//!
//! Status: program identity VERIFIED + pinned (`RAYDIUM_CPMM` in
//! `consts.rs`, drift-checked). Instruction/account layout NOT yet verified
//! against the current CPMM IDL — so [`VenueAdapter::build_swap`] refuses
//! with `LayoutUnverified` until that work lands with tests.
//!
//! What IS safe to use today: pool-discovery facts. A Raydium CPMM pool is
//! owned by the CPMM program; detection code can filter program accounts by
//! owner without knowing the field layout, which is enough for "does a CPMM
//! pool exist for this launch?" style questions in risk_intel.

use solana_sdk::pubkey::Pubkey;

use crate::consts::RAYDIUM_CPMM;
use crate::venues::VenueAdapter;

/// Raydium CPMM adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RaydiumCpmm {
    enabled: bool,
}

impl RaydiumCpmm {
    /// Enabled adapter (layout still unverified — building stays refused).
    pub fn new() -> Self {
        RaydiumCpmm { enabled: true }
    }

    /// Config-driven constructor.
    pub fn with_enabled(enabled: bool) -> Self {
        RaydiumCpmm { enabled }
    }

    /// True when `account_owner` is the CPMM program — the only layout-free
    /// fact this adapter exposes until the IDL pass completes.
    pub fn is_pool_owner(&self, account_owner: &Pubkey) -> bool {
        account_owner == &*RAYDIUM_CPMM
    }
}

impl VenueAdapter for RaydiumCpmm {
    fn id(&self) -> &'static str {
        "raydium_cpmm"
    }

    fn label(&self) -> &'static str {
        "Raydium CPMM"
    }

    fn program_id(&self) -> &Pubkey {
        &RAYDIUM_CPMM
    }

    fn layout_verified(&self) -> bool {
        // Flipping this to true requires: the current CPMM IDL reviewed,
        // pool/account layouts decoded here with unit tests against a
        // recorded account fixture, and a devnet simulation of a tiny swap
        // recorded under evidence/live/. See venues/mod.rs.
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
    fn pool_owner_check_matches_the_pinned_program() {
        let venue = RaydiumCpmm::new();
        assert!(venue.is_pool_owner(&RAYDIUM_CPMM));
        assert!(!venue.is_pool_owner(&Pubkey::new_unique()));
    }

    #[test]
    fn disabled_adapter_reports_disabled() {
        assert!(!RaydiumCpmm::with_enabled(false).enabled());
        assert!(RaydiumCpmm::with_enabled(true).enabled());
    }
}
