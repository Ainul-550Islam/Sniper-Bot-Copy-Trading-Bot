//! Venue adapters beyond the built-in pump.fun / PumpSwap / Raydium AMM v4
//! paths (GAP-MAP v2, P2).
//!
//! The GAP item is explicit: adapters only AFTER confirming current program
//! ids, layouts and volume. This module therefore separates the two halves
//! of venue knowledge:
//!
//! * **Program identity** — verified and pinned (see the `const` blocks in
//!   each adapter; every id is also in `scripts/protocol-pins.json` and
//!   guarded by `check-protocol-drift.sh`).
//! * **Instruction/account layouts** — NOT yet verified against the current
//!   IDLs for these venues. Until an adapter's [`VenueAdapter::layout_verified`]
//!   returns `true`, [`VenueAdapter::build_swap`] MUST refuse with
//!   [`VenueError::LayoutUnverified`]. Shipping unverified layouts is how
//!   funds get lost; the type system makes the gate unmissable.
//!
//! Current adapters:
//! * `raydium_cpmm` — Raydium CPMM (constant-product market maker pools).
//! * `meteora`      — Meteora DLMM and Meteora DAMM v1.
//!
//! Not included yet, deliberately: LaunchLab (const exists in
//! `crate::consts::RAYDIUM_LAUNCHLAB`, but its pool layout needs the same
//! verification pass first) and any venue whose 2026 docs we have not read.

pub mod meteora;
pub mod raydium_cpmm;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use bot_core::error::{BotError, BotResult};

/// Why a venue adapter refused to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VenueError {
    /// The adapter's account/instruction layout has not been verified
    /// against the venue's CURRENT IDL — building is refused until it is.
    LayoutUnverified,
    /// The requested pair has no pool on this venue.
    NoPool,
    /// The venue is disabled by configuration.
    Disabled,
}

impl VenueError {
    /// Uniform error surface for callers.
    pub fn into_bot_error(self, venue: &'static str) -> BotError {
        match self {
            VenueError::LayoutUnverified => BotError::config(format!(
                "venue {venue}: instruction layout unverified — refusing to build (verify the current IDL first)"
            )),
            VenueError::NoPool => BotError::invalid(format!("venue {venue}: no pool for pair")),
            VenueError::Disabled => BotError::config(format!("venue {venue}: disabled")),
        }
    }
}

/// A swap the adapter is asked to build. Amounts are RAW token units; which
/// side is the input depends on `side`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueSwapRequest {
    /// Base mint (the traded token).
    pub base_mint: Pubkey,
    /// Quote mint (SOL/WSOL/etc.).
    pub quote_mint: Pubkey,
    /// true = buy base with quote; false = sell base for quote.
    pub buy: bool,
    /// Input amount in raw units of the input mint.
    pub amount_in: u64,
    /// Minimum output in raw units of the output mint (slippage guard).
    pub min_out: u64,
}

/// Static knowledge about a venue: identity + verification status. Swap
/// building is intentionally NOT on this trait until a layout is verified;
/// adapters expose their own verified builders when ready.
pub trait VenueAdapter {
    /// Stable label for telemetry/gates (`"raydium_cpmm"`, `"meteora_dlmm"`).
    fn id(&self) -> &'static str;

    /// Human-readable name.
    fn label(&self) -> &'static str;

    /// The venue's on-chain program id (pinned + drift-checked).
    fn program_id(&self) -> &Pubkey;

    /// Whether this adapter's instruction/account layout has been verified
    /// against the venue's CURRENT IDL. Until true, building is refused.
    fn layout_verified(&self) -> bool;

    /// Whether the venue is enabled in configuration.
    fn enabled(&self) -> bool;

    /// Build a swap instruction. Default implementation enforces the
    /// verification gate; verified adapters override it.
    fn build_swap(&self, _request: &VenueSwapRequest) -> BotResult<Vec<u8>> {
        Err(VenueError::LayoutUnverified.into_bot_error(self.id()))
    }
}

/// The registry of all known venue adapters, in routing-preference order.
/// Detection consumers (launch gates, market service) iterate this to answer
/// "which venue could serve this pair?".
pub struct VenueRegistry {
    adapters: Vec<Box<dyn VenueAdapter>>,
}

impl VenueRegistry {
    /// The default registry: every shipped adapter, preference order.
    pub fn default_registry() -> Self {
        VenueRegistry {
            adapters: vec![
                Box::new(raydium_cpmm::RaydiumCpmm::new()),
                Box::new(meteora::MeteoraDlmm::new()),
                Box::new(meteora::MeteoraDammV1::new()),
            ],
        }
    }

    /// All adapters.
    pub fn all(&self) -> &[Box<dyn VenueAdapter>] {
        &self.adapters
    }

    /// Adapters whose layout is verified AND which are enabled — the only
    /// ones a caller may build transactions for.
    pub fn buildable(&self) -> Vec<&dyn VenueAdapter> {
        self.adapters
            .iter()
            .filter(|v| v.layout_verified() && v.enabled())
            .map(|b| b.as_ref() as &dyn VenueAdapter)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unverified_adapters_refuse_to_build() {
        for adapter in VenueRegistry::default_registry().all() {
            let req = VenueSwapRequest {
                base_mint: Pubkey::new_unique(),
                quote_mint: Pubkey::new_unique(),
                buy: true,
                amount_in: 1_000,
                min_out: 1,
            };
            let err = adapter.build_swap(&req).unwrap_err();
            assert!(
                err.to_string().contains("unverified"),
                "{} must refuse until layout_verified",
                adapter.id()
            );
            assert!(!adapter.layout_verified());
        }
    }

    #[test]
    fn nothing_is_buildable_until_layouts_are_verified() {
        // This test is the tripwire: when an adapter's layout gets verified,
        // it must be a DELIBERATE change that shows up here.
        assert!(VenueRegistry::default_registry().buildable().is_empty());
    }

    #[test]
    fn program_ids_match_the_pins() {
        use crate::consts;
        let reg = VenueRegistry::default_registry();
        let cpmm = reg.all().iter().find(|v| v.id() == "raydium_cpmm").unwrap();
        assert_eq!(*cpmm.program_id(), *consts::RAYDIUM_CPMM);
        let dlmm = reg.all().iter().find(|v| v.id() == "meteora_dlmm").unwrap();
        assert_eq!(*dlmm.program_id(), *consts::METEORA_DLMM);
        let damm = reg.all().iter().find(|v| v.id() == "meteora_damm_v1").unwrap();
        assert_eq!(*damm.program_id(), *consts::METEORA_DAMM_V1);
    }
}
