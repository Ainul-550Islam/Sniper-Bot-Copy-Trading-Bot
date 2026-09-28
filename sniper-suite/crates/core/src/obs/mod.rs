//! Observability primitives shared by every crate: a dependency-free
//! Prometheus [`metrics`] registry and a liveness/readiness [`health`]
//! registry.
//!
//! The server wires these to HTTP (`/health`, `/ready`, `/metrics`), to the
//! [`crate::events::EventBus`] (event-derived counters and latency
//! histograms) and to a periodic state sampler (gauges mirrored from
//! [`crate::state::AppState`]). Instrumentation inside `solana-kit` records
//! RPC and websocket metrics directly into the process-wide registry
//! ([`metrics::global`]).

pub mod health;
pub mod metrics;

pub use health::{ComponentStatus, HealthRegistry, HealthReport};
pub use metrics::{Counter, Gauge, Histogram, Registry};
