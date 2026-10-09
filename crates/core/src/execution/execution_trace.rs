//! Tenant-safe execution tracing metadata (STEP 3 file 14).
//!
//! An [`ExecutionTrace`] carries the correlation identity of one execution
//! through every stage: the trace id minted when the request entered, the
//! correlation id it derives from (request / job / recovery), and the
//! timestamp. It is metadata ONLY — ids and time, never payloads, keys or
//! order details — so it is safe in every log, metric label and audit
//! event it travels with.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Correlation prefix for a request-originated trace.
pub const REQUEST_ORIGIN: &str = "req";
/// Correlation prefix for a background-job-originated trace.
pub const JOB_ORIGIN: &str = "job";
/// Correlation prefix for a recovery-originated trace.
pub const RECOVERY_ORIGIN: &str = "rec";
/// Correlation prefix for a websocket-originated trace.
pub const STREAM_ORIGIN: &str = "ws";

/// The tracing/correlation identity of one execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionTrace {
    trace_id: Uuid,
    correlation_id: String,
    origin: String,
    started_at: DateTime<Utc>,
}

impl ExecutionTrace {
    /// Mint a fresh trace under an explicit origin (`REQUEST_ORIGIN`, …).
    pub fn new(origin: &'static str) -> Self {
        ExecutionTrace {
            trace_id: Uuid::new_v4(),
            correlation_id: format!("{origin}-{}", Uuid::new_v4()),
            origin: origin.to_string(),
            started_at: Utc::now(),
        }
    }

    /// A request-originated trace.
    pub fn for_request() -> Self {
        ExecutionTrace::new(REQUEST_ORIGIN)
    }

    /// A background-job-originated trace.
    pub fn for_job() -> Self {
        ExecutionTrace::new(JOB_ORIGIN)
    }

    /// A recovery-originated trace.
    pub fn for_recovery() -> Self {
        ExecutionTrace::new(RECOVERY_ORIGIN)
    }

    /// Derive a child trace that shares the parent's correlation id (the
    /// same logical operation, a new execution attempt).
    pub fn derive_child(&self) -> Self {
        ExecutionTrace {
            trace_id: Uuid::new_v4(),
            correlation_id: self.correlation_id.clone(),
            origin: self.origin.clone(),
            started_at: Utc::now(),
        }
    }

    /// The unique id of THIS execution attempt.
    pub fn trace_id(&self) -> Uuid {
        self.trace_id
    }

    /// The stable correlation id across attempts of one operation.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Where the execution originated.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// When the trace was minted.
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    /// Structured-log fields (the allowlist every stage logs the same
    /// way). Values are ids only — nothing secret can enter through here.
    pub fn fields(&self) -> [(&'static str, String); 3] {
        [
            ("trace_id", self.trace_id.to_string()),
            ("correlation_id", self.correlation_id.clone()),
            ("origin", self.origin.clone()),
        ]
    }
}

impl fmt::Display for ExecutionTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.correlation_id, self.trace_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_traces_are_unique_but_well_formed() {
        let a = ExecutionTrace::for_request();
        let b = ExecutionTrace::for_request();
        assert_ne!(a.trace_id(), b.trace_id());
        assert_ne!(a.correlation_id(), b.correlation_id());
        assert!(a.correlation_id().starts_with("req-"));
        assert_eq!(a.origin(), REQUEST_ORIGIN);
    }

    #[test]
    fn origins_select_the_prefix() {
        assert!(ExecutionTrace::for_job()
            .correlation_id()
            .starts_with("job-"));
        assert!(ExecutionTrace::for_recovery()
            .correlation_id()
            .starts_with("rec-"));
        assert!(ExecutionTrace::new(STREAM_ORIGIN)
            .correlation_id()
            .starts_with("ws-"));
    }

    #[test]
    fn children_share_correlation_but_not_identity() {
        let parent = ExecutionTrace::for_request();
        let child = parent.derive_child();
        assert_eq!(parent.correlation_id(), child.correlation_id());
        assert_ne!(parent.trace_id(), child.trace_id());
        assert_eq!(parent.origin(), child.origin());
    }

    #[test]
    fn fields_are_the_log_allowlist() {
        let t = ExecutionTrace::for_job();
        let fields = t.fields();
        assert_eq!(fields[0].0, "trace_id");
        assert_eq!(fields[1].0, "correlation_id");
        assert_eq!(fields[2].0, "origin");
        assert!(fields.iter().all(|(_, v)| !v.is_empty()));
        // Only ids — the display and fields can never carry a payload.
        assert!(t.to_string().starts_with("job-"));
    }
}
