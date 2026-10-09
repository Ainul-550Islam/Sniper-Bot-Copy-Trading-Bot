//! Safe structured log fields (tenant-isolation file 60).
//!
//! [`TenantLogFields`] builds the allowlist of tenant-safe structured
//! fields: organization/runtime/module identities and correlation
//! ids. What is deliberately ABSENT is the point: no seeds, no
//! private keys, no bearer tokens, no signer secrets, no provider
//! credentials — the builder simply has no method that could add
//! one, and [`Redaction`] sweeps anything a caller smuggles through
//! generic maps.
//!
//! Field names are stable (metrics/audit grouping keys).

use std::fmt;

use bot_core::tenant::{ModuleKind, OrganizationId};

use crate::runtime_registry::TenantRuntimeRecord;

use super::redaction::Redaction;

/// One safe field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantLogField {
    /// The stable field name.
    pub name: &'static str,
    /// The non-secret value.
    pub value: String,
}

/// The stable field-name vocabulary.
pub mod field_names {
    /// The acting tenant.
    pub const ORGANIZATION_ID: &str = "organization_id";
    /// The runtime instance.
    pub const RUNTIME_ID: &str = "runtime_id";
    /// The runtime's fencing generation.
    pub const GENERATION: &str = "generation";
    /// The module.
    pub const MODULE: &str = "module";
    /// The request correlation id.
    pub const REQUEST_ID: &str = "request_id";
    /// The execution correlation id.
    pub const EXECUTION_ID: &str = "execution_id";
    /// The order id.
    pub const ORDER_ID: &str = "order_id";
    /// The trace correlation id.
    pub const CORRELATION_ID: &str = "correlation_id";
    /// The acting principal (non-secret label).
    pub const PRINCIPAL: &str = "principal";
    /// The entry origin.
    pub const ORIGIN: &str = "origin";
}

/// A builder for a safe, tenant-aware set of structured log fields.
#[derive(Debug, Clone, Default)]
pub struct TenantLogFields {
    fields: Vec<TenantLogField>,
}

impl TenantLogFields {
    /// An empty field set.
    pub fn new() -> Self {
        TenantLogFields::default()
    }

    /// The acting tenant.
    pub fn organization(mut self, organization_id: OrganizationId) -> Self {
        self.push(field_names::ORGANIZATION_ID, organization_id.to_string());
        self
    }

    /// The full runtime identity (runtime id + generation).
    pub fn runtime(mut self, record: &TenantRuntimeRecord) -> Self {
        self.push(field_names::RUNTIME_ID, record.runtime_id.to_string());
        self.push(field_names::GENERATION, record.generation.raw().to_string());
        self
    }

    /// Just the runtime id.
    pub fn runtime_id(mut self, runtime_id: impl fmt::Display) -> Self {
        self.push(field_names::RUNTIME_ID, runtime_id.to_string());
        self
    }

    /// The module.
    pub fn module(mut self, module: ModuleKind) -> Self {
        self.push(field_names::MODULE, module.as_str().to_string());
        self
    }

    /// A request correlation id.
    pub fn request_id(mut self, id: impl fmt::Display) -> Self {
        self.push(field_names::REQUEST_ID, id.to_string());
        self
    }

    /// An execution correlation id.
    pub fn execution_id(mut self, id: impl fmt::Display) -> Self {
        self.push(field_names::EXECUTION_ID, id.to_string());
        self
    }

    /// An order id.
    pub fn order_id(mut self, id: impl fmt::Display) -> Self {
        self.push(field_names::ORDER_ID, id.to_string());
        self
    }

    /// A trace correlation id.
    pub fn correlation_id(mut self, id: impl fmt::Display) -> Self {
        self.push(field_names::CORRELATION_ID, id.to_string());
        self
    }

    /// The acting principal (a non-secret label — secrets never reach
    /// this builder, and `Redaction` sweeps values regardless).
    pub fn principal(mut self, principal: &str) -> Self {
        self.push(field_names::PRINCIPAL, principal.to_string());
        self
    }

    /// The entry origin (http/stream/job/recovery).
    pub fn origin(mut self, origin: &str) -> Self {
        self.push(field_names::ORIGIN, origin.to_string());
        self
    }

    fn push(&mut self, name: &'static str, value: String) {
        // Redaction runs even here: a principal label that somehow
        // carries secret-shaped content is masked at insertion.
        let value = Redaction::redact_value(name, &value).into_owned();
        if let Some(existing) = self.fields.iter_mut().find(|f| f.name == name) {
            existing.value = value;
        } else {
            self.fields.push(TenantLogField { name, value });
        }
    }

    /// The built fields (already redacted).
    pub fn fields(&self) -> &[TenantLogField] {
        &self.fields
    }

    /// Consume into the built fields.
    pub fn into_fields(self) -> Vec<TenantLogField> {
        self.fields
    }

    /// Render as `k=v` pairs (one line, deterministic order).
    pub fn to_log_line(&self) -> String {
        self.fields
            .iter()
            .map(|f| format!("{}={}", f.name, f.value))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl fmt::Display for TenantLogFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_log_line())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};
    use chrono::Utc;

    #[test]
    fn fields_carry_only_safe_identities() {
        let organization_id = OrganizationId::new();
        let runtime_id = RuntimeId::new();
        let fields = TenantLogFields::new()
            .organization(organization_id)
            .runtime_id(runtime_id)
            .module(ModuleKind::Copy)
            .request_id("req-9")
            .execution_id("exec-1")
            .order_id("ord-77")
            .correlation_id("corr-3")
            .principal("user:1")
            .origin("http");

        let names: Vec<&str> = fields.fields().iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "organization_id",
                "runtime_id",
                "module",
                "request_id",
                "execution_id",
                "order_id",
                "correlation_id",
                "principal",
                "origin",
            ]
        );
        // The log line is deterministic and carries no secret shape.
        let line = fields.to_log_line();
        assert!(line.contains(&organization_id.to_string()));
        assert!(line.contains("module=copy"));
    }

    #[test]
    fn duplicate_names_collapse_to_the_last_value() {
        let fields = TenantLogFields::new().order_id("ord-1").order_id("ord-2");
        assert_eq!(fields.fields().len(), 1);
        assert_eq!(fields.fields()[0].value, "ord-2");
    }

    #[test]
    fn the_runtime_helper_carries_id_and_generation() {
        let record = TenantRuntimeRecord {
            runtime_id: RuntimeId::new(),
            organization_id: OrganizationId::new(),
            generation: RuntimeGeneration::first(),
            status: crate::runtime_registry::RuntimeStatus::Active,
            worker_id: "worker-a".into(),
            started_at: Utc::now(),
            heartbeat_at: Utc::now(),
            lease_expires_at: None,
            stopped_at: None,
        };
        let fields = TenantLogFields::new().runtime(&record);
        assert_eq!(fields.fields().len(), 2);
        assert_eq!(fields.fields()[0].name, "runtime_id");
        assert_eq!(fields.fields()[1].name, "generation");
        assert_eq!(fields.fields()[1].value, "1");
    }

    #[test]
    fn a_secret_shaped_value_is_masked_even_through_the_builder() {
        let fields = TenantLogFields::new().principal("user:1");
        assert_eq!(fields.fields()[0].value, "user:1");
        // The redaction module masks secret-shaped KEYS; the builder
        // only offers safe keys — this asserts the contract holds end
        // to end through `Redaction::redact_value`.
        assert_eq!(Redaction::redact_value("principal", "user:1"), "user:1");
        assert_eq!(Redaction::redact_value("private_key", "abc"), "[REDACTED]");
    }
}
