#![allow(dead_code)]
//! sniper-suite library crate (Batch 7).
//! Exposes ops/solana/staking/billing/custody contracts for integration tests.
//! Binary crate (main.rs) reuses same modules; this lib is for `cargo test --test ...` harnesses.

pub mod ops;
pub mod solana {
    pub mod connection_contract;
    pub mod geyser_contract;
}
pub mod staking {
    pub mod deployment_contract;
    pub mod validator_contract;
}
pub mod billing {
    pub mod live_provider_contract;
    pub mod live_provider_fixture;
    pub mod paddle_adapter;
    pub mod provider_registry;
    pub mod stripe_adapter;
}
pub mod custody {
    pub mod live_provider_contract;
    pub mod live_provider_fixture;
}
