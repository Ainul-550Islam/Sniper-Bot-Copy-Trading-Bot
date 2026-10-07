#![recursion_limit = "256"]
#![allow(dead_code)]
#![allow(clippy::result_large_err)]
//! sniper-suite library crate (Batch 7, §G/§H Batch 8).
//!
//! The binary crate (`main.rs`) owns the runtime; this lib mirrors the
//! SAME module tree so `cargo test --test ...` harnesses can exercise the
//! real production code (ops/solana/staking/billing/custody contracts,
//! the SaaS control plane, the trading data plane) through
//! `sniper_suite::*` instead of test-only copies. Every module here is
//! the same file on disk the binary compiles — there is exactly one
//! implementation of each concern.

pub mod accounting;
pub mod api;
pub mod backup;
pub mod dashboard;
pub mod ha;
pub mod module_runtime;
pub mod obs;
pub mod openapi_team_security;
pub mod openapi_trading_data_plane;
pub mod ops;
pub mod persist;
pub mod provisioning;
pub mod recon;
pub mod runtime_registry;
pub mod saas;
pub mod tenant;
pub mod tenant_background;
pub mod tenant_config;
pub mod tenant_observability;
pub mod tenant_streams;
pub mod trading_data_plane;
// TASK 7B — response-header hardening and the authenticated, tenant-scoped
// event stream. The two files live under src/security/; this inline parent
// module keeps the mandated file tree (no extra security/mod.rs).
// BATCH — production-safe CORS and reusable tenant-context extraction.
pub mod security {
    pub mod cors_policy;
    pub mod headers;
    pub mod legacy_websocket_guard;
    pub mod security_headers;
    pub mod tenant_context;
    pub mod websocket;
}
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
pub mod custody;
pub mod ws;
