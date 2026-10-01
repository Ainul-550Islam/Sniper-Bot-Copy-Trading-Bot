//! Tenant execution authorization gateway (STEP 3 files 13–24 + the
//! tenant-isolation context layer).
//!
//! Everything a tenant tries to execute passes through this module's
//! [`gateway::TenantExecutionGateway`] before it may reach the module
//! engines. The gateway runs a FIXED, ordered chain of guards and either
//! issues a core [`bot_core::execution::TenantExecutionContext`] (the
//! same typed proof the engines already consume) or denies with a
//! machine-readable reason:
//!
//! ```text
//! request ─→ tenant_guard ─→ entitlement_guard ─→ module_guard ─→ mode_guard
//!         ─→ binding_guard ─→ risk_guard ─→ fence_guard ─→ context issue
//! ```
//!
//! The context layer in front of the chain:
//!
//! ```text
//! credential ─→ context_resolver ─→ TenantContext ─→ context_guard
//!            ─→ runtime_context (live runtime + generation)
//!            ─→ gateway.authorize_in_context ─→ TenantExecutionContext
//! ```
//!
//! | file | concern |
//! |---|---|
//! | `request.rs` | [`request::TenantExecutionRequest`] — what the tenant asks for |
//! | `decision.rs` | [`decision::TenantDecision`] — allow/deny + the closed deny vocabulary |
//! | `context.rs` | [`context::TenantContext`] — the typed tenant execution context |
//! | `context_resolver.rs` | [`context_resolver::TenantContextResolver`] — principal → tenant (missing/ambiguous rejected) |
//! | `context_guard.rs` | [`context_guard::check`] — a valid context before tenant-sensitive work |
//! | `registry.rs` | [`registry::TenantBindingRegistry`] — wallet/signer bindings per tenant |
//! | `tenant_guard.rs` | organization state (active/suspended/closed) — the lifecycle gate |
//! | `entitlement_guard.rs` | plan entitlements (module + live trading) |
//! | `module_guard.rs` | module enablement under the tenant's config |
//! | `mode_guard.rs` | trading mode (paper/simulate/live) |
//! | `binding_guard.rs` | wallet + signer ownership and activity |
//! | `wallet_guard.rs` | [`wallet_guard::WalletGuard`] — the context-aware wallet binding guard |
//! | `signer_guard.rs` | [`signer_guard::SignerGuard`] — the context-aware signer binding guard |
//! | `risk_guard.rs` | position size / slippage vs effective limits |
//! | `fence_guard.rs` | runtime fence (the tenant's live runtime, current generation) |
//! | `runtime_context.rs` | [`runtime_context::RuntimeContext`] — resolve the current live runtime + generation |
//! | `runtime_cache.rs` | [`runtime_cache::RuntimeMetadataCache`] — cached runtime metadata (never authorizes) |
//! | `gateway.rs` | the ordered chain + context issuance (`authorize` / `authorize_in_context`) |
//!
//! Disciplines inherited from the codebase:
//!
//! * **Fail closed.** Any guard error (storage down, config unreadable)
//!   is a DENY, never a "best effort allow".
//! * **Closed vocabularies.** Deny reasons are enum values, not strings.
//! * **No secrets.** The request carries references (labels, key refs),
//!   never key material.
//! * **One proof.** The ONLY thing that leaves this module on success is
//!   the core-issued context — guards do not invent their own tokens.
//! * **Everything metered.** Every decision passes through the observability
//!   hook (see `decision.rs`) so the audit trail and metrics see the same
//!   verdicts the caller sees.

pub mod binding_guard;
pub mod context;
pub mod context_guard;
pub mod context_resolver;
pub mod decision;
pub mod entitlement_guard;
pub mod fence_guard;
pub mod gateway;
pub mod mode_guard;
pub mod module_guard;
pub mod registry;
pub mod request;
pub mod risk_guard;
pub mod runtime_cache;
pub mod runtime_context;
pub mod signer_guard;
pub mod tenant_guard;
pub mod wallet_guard;

pub use context::{ContextError, ContextOrigin, TenantContext};
pub use context_guard::check as check_context;
pub use context_resolver::{ContextResolutionError, TenantContextResolver};
pub use decision::{DenyReason, GuardOutcome, TenantDecision};
pub use gateway::{ExecutionInputs, TenantExecutionGateway};
pub use registry::{MemoryTenantBindingRegistry, PgTenantBindingRegistry, TenantBindingRegistry};
pub use request::TenantExecutionRequest;
pub use runtime_cache::{RuntimeMetadataCache, DEFAULT_RUNTIME_CACHE_TTL};
pub use runtime_context::{RuntimeContext, RuntimeContextError};
pub use signer_guard::SignerGuard;
pub use wallet_guard::WalletGuard;
