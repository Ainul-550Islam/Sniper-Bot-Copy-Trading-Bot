//! Execution scope (STEP 3 file 13).
//!
//! The [`ExecutionScope`] is the ONE value that identifies "who is
//! executing, where, on whose behalf": organization, runtime instance,
//! fencing generation, module and trading environment. It is built once
//! per authorized execution and every downstream stage (risk, signing,
//! broadcast, persistence) re-derives or verifies against it — a stage
//! that cannot see the scope structurally cannot execute.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::models::ExecutionMode;
use crate::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};

/// The coherent identity of one tenant execution path.
///
/// All fields are public data (ids and enums); nothing here is a secret.
/// Coherence is enforced at construction: [`ExecutionScope::new`] requires
/// a non-nil runtime id, because a scope without a runtime cannot be
/// fenced and therefore must not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionScope {
    organization_id: OrganizationId,
    runtime_id: RuntimeId,
    generation: RuntimeGeneration,
    module: ModuleKind,
    mode: ExecutionMode,
}

impl ExecutionScope {
    /// Build a scope. Fails closed on a nil runtime id (an unfenceable
    /// scope would be an authorization hole, not a convenience).
    pub fn new(
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
        module: ModuleKind,
        mode: ExecutionMode,
    ) -> Result<Self, ScopeError> {
        if runtime_id.is_nil() {
            return Err(ScopeError::NilRuntime);
        }
        Ok(ExecutionScope {
            organization_id,
            runtime_id,
            generation,
            module,
            mode,
        })
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The runtime instance executing.
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    /// The runtime's fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }

    /// The trading module.
    pub fn module(&self) -> ModuleKind {
        self.module
    }

    /// The trading environment.
    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    /// Would this scope move real funds?
    pub fn is_live(&self) -> bool {
        self.mode.is_live()
    }

    /// A stable, human-readable fingerprint input (see
    /// [`super::execution_authority`] for the authority fingerprint).
    pub fn describe(&self) -> String {
        format!(
            "org={} runtime={} {} module={} mode={}",
            self.organization_id,
            self.runtime_id,
            self.generation,
            self.module.as_str(),
            self.mode.as_str(),
        )
    }
}

impl fmt::Display for ExecutionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Why a scope could not be built. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    /// The nil runtime id was supplied.
    NilRuntime,
}

impl ScopeError {
    /// Stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeError::NilRuntime => "nil_runtime",
        }
    }
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeError::NilRuntime => {
                write!(
                    f,
                    "execution scope rejected: nil runtime id is not fenceable"
                )
            }
        }
    }
}

impl std::error::Error for ScopeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BotModule;
    use crate::tenant::{RuntimeGeneration, RuntimeId};

    fn scope() -> Result<ExecutionScope, ScopeError> {
        ExecutionScope::new(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            BotModule::Sniper,
            ExecutionMode::Paper,
        )
    }

    #[test]
    fn builds_and_exposes_every_field() {
        let s = scope().unwrap();
        assert_eq!(s.module(), BotModule::Sniper);
        assert_eq!(s.generation(), RuntimeGeneration::first());
        assert!(!s.is_live());
        assert!(s.describe().contains("module=sniper"));
    }

    #[test]
    fn nil_runtime_is_rejected_fail_closed() {
        let err = ExecutionScope::new(
            OrganizationId::new(),
            RuntimeId::from(uuid::Uuid::nil()),
            RuntimeGeneration::first(),
            BotModule::Copy,
            ExecutionMode::Live,
        )
        .unwrap_err();
        assert_eq!(err, ScopeError::NilRuntime);
        assert_eq!(err.as_str(), "nil_runtime");
    }

    #[test]
    fn live_scopes_report_live() {
        let s = ExecutionScope::new(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            BotModule::Polymarket,
            ExecutionMode::Live,
        )
        .unwrap();
        assert!(s.is_live());
    }

    #[test]
    fn scopes_are_hashable_and_serializable() {
        let s = scope().unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: ExecutionScope = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
