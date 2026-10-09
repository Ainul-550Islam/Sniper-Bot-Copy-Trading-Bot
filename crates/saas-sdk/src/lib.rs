#![forbid(unsafe_code)]
//! Public Rust SDK for the SaaS control-plane API (BATCH file 24).
//!
//! Keep dependencies minimal. Target stable public API usage, not server internals.
//! Do not copy server implementation into SDK.

pub mod alerts;
pub mod backtest;
pub mod billing;
pub mod client;
pub mod commercial;
pub mod custody;
pub mod error;
pub mod models;
pub mod ops;
pub mod portfolio;
pub mod risk;
pub mod strategy;
pub mod support;
pub mod team_security;

pub use client::{SaasClient, SaasClientBuilder};
pub use error::{SdkError, SdkErrorKind};
pub use models::{
    ApiKeyMetadata, BillingCheckoutRequest, BillingCheckoutResponse, InvoiceView, OrganizationView,
    SessionResponse, TenantStatus, UserProfile, WalletBindingView,
};

/// SDK version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Re-export client builder as primary entrypoint.
pub fn builder() -> SaasClientBuilder {
    SaasClientBuilder::new()
}
