//! Execution authority (STEP 3 file 12).
//!
//! An [`ExecutionAuthority`] is the PROOF that one execution request
//! passed the full guard chain — it cannot be fabricated, only ISSUED by
//! completing every mandated check, in the mandated order, inside
//! [`AuthorityChecklist`]. The execution engine accepts a tenant context
//! only when it carries an authority whose fingerprint matches the
//! execution scope (organization + runtime + generation + module + mode).
//!
//! The mandated order (PROMPT 2/10 PHASE 2) is fixed in
//! [`AUTHORITY_CHECK_ORDER`]; the checklist enforces it mechanically: a
//! check can only be recorded when its predecessor is already present,
//! and [`AuthorityChecklist::finish`] fails when any check is missing.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::sha256_hex;

use super::execution_scope::ExecutionScope;

/// The mandated guard order. Index 0 runs first; the composed guard may
/// not record a check out of order.
pub const AUTHORITY_CHECK_ORDER: [&str; 11] = [
    "authenticated_principal",
    "tenant_context",
    "tenant_lifecycle",
    "runtime_exists",
    "runtime_active",
    "runtime_generation",
    "module_entitlement",
    "tenant_config",
    "wallet_binding",
    "signer_binding",
    "risk_permission",
];

/// One recorded, passed check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityCheck {
    /// Which check (a label from [`AUTHORITY_CHECK_ORDER`]).
    pub name: String,
    /// When it passed.
    pub passed_at: DateTime<Utc>,
}

impl AuthorityCheck {
    /// The label this check was recorded under.
    pub fn label(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for AuthorityCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.passed_at.to_rfc3339())
    }
}

/// Why an authority could not be issued. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityError {
    /// A check was recorded out of the mandated order.
    OutOfOrder {
        /// The check that was attempted.
        attempted: &'static str,
        /// The check that must pass first.
        requires: &'static str,
    },
    /// A check unknown to the mandated order was attempted.
    UnknownCheck(&'static str),
    /// [`AuthorityChecklist::finish`] ran with checks still missing.
    MissingChecks(Vec<&'static str>),
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthorityError::OutOfOrder {
                attempted,
                requires,
            } => write!(
                f,
                "authority check out of order: {attempted} requires {requires} first"
            ),
            AuthorityError::UnknownCheck(name) => {
                write!(f, "unknown authority check: {name}")
            }
            AuthorityError::MissingChecks(missing) => {
                write!(f, "authority incomplete, missing: {}", missing.join(", "))
            }
        }
    }
}

impl std::error::Error for AuthorityError {}

/// The ordered record of passed checks. Build one per request.
#[derive(Debug, Clone, Default)]
pub struct AuthorityChecklist {
    checks: Vec<AuthorityCheck>,
}

impl AuthorityChecklist {
    /// An empty checklist.
    pub fn new() -> Self {
        AuthorityChecklist::default()
    }

    /// The checks recorded so far, in order.
    pub fn recorded(&self) -> &[AuthorityCheck] {
        &self.checks
    }

    /// Record a passed check. Fails (without mutating) when the check is
    /// unknown or its mandated predecessor has not been recorded yet.
    pub fn record(&mut self, name: &'static str, at: DateTime<Utc>) -> Result<(), AuthorityError> {
        let position = AUTHORITY_CHECK_ORDER
            .iter()
            .position(|c| *c == name)
            .ok_or(AuthorityError::UnknownCheck(name))?;
        if position > 0 {
            let requires = AUTHORITY_CHECK_ORDER[position - 1];
            if !self.checks.iter().any(|c| c.name == requires) {
                return Err(AuthorityError::OutOfOrder {
                    attempted: name,
                    requires,
                });
            }
        }
        // Duplicates are idempotent re-records (guard retries): keep the
        // first timestamp, which is when the check first passed.
        if self.checks.iter().any(|c| c.name == name) {
            return Ok(());
        }
        self.checks.push(AuthorityCheck {
            name: name.to_string(),
            passed_at: at,
        });
        Ok(())
    }

    /// Have ALL mandated checks been recorded, in order?
    pub fn is_complete(&self) -> bool {
        self.checks.len() == AUTHORITY_CHECK_ORDER.len()
            && self
                .checks
                .iter()
                .zip(AUTHORITY_CHECK_ORDER.iter())
                .all(|(recorded, mandated)| recorded.name == *mandated)
    }

    /// Issue the authority for a scope. Fails while any check is missing.
    pub fn finish(
        self,
        scope: &ExecutionScope,
        at: DateTime<Utc>,
    ) -> Result<ExecutionAuthority, AuthorityError> {
        if !self.is_complete() {
            let missing: Vec<&'static str> = AUTHORITY_CHECK_ORDER
                .iter()
                .copied()
                .filter(|name| !self.checks.iter().any(|c| c.name == *name))
                .collect();
            return Err(AuthorityError::MissingChecks(missing));
        }
        Ok(ExecutionAuthority {
            fingerprint: ExecutionAuthority::fingerprint_of(scope),
            granted_at: at,
            checks: self.checks,
        })
    }
}

/// The proof object. Constructed only via [`AuthorityChecklist::finish`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAuthority {
    fingerprint: String,
    granted_at: DateTime<Utc>,
    checks: Vec<AuthorityCheck>,
}

impl ExecutionAuthority {
    /// The scope fingerprint this authority authorizes.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// When the last check passed and the authority was issued.
    pub fn granted_at(&self) -> DateTime<Utc> {
        self.granted_at
    }

    /// The recorded checks, in the mandated order.
    pub fn checks(&self) -> &[AuthorityCheck] {
        &self.checks
    }

    /// Does this authority authorize exactly this scope?
    pub fn authorizes(&self, scope: &ExecutionScope) -> bool {
        self.fingerprint == Self::fingerprint_of(scope)
    }

    /// The deterministic fingerprint of a scope (sha256 of the public
    /// identity fields; no secrets are involved).
    pub fn fingerprint_of(scope: &ExecutionScope) -> String {
        sha256_hex(&scope.describe())
    }
}

impl fmt::Display for ExecutionAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "authority({} checks, granted {})",
            self.checks.len(),
            self.granted_at.to_rfc3339()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BotModule, ExecutionMode};
    use crate::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

    fn scope() -> ExecutionScope {
        ExecutionScope::new(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            BotModule::Sniper,
            ExecutionMode::Paper,
        )
        .unwrap()
    }

    fn complete_checklist() -> AuthorityChecklist {
        let mut c = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            c.record(name, now).unwrap_or_else(|e| panic!("{e}"));
        }
        c
    }

    #[test]
    fn the_mandated_order_is_encoded_once() {
        assert_eq!(AUTHORITY_CHECK_ORDER.len(), 11);
        assert_eq!(AUTHORITY_CHECK_ORDER[0], "authenticated_principal");
        assert_eq!(AUTHORITY_CHECK_ORDER[10], "risk_permission");
    }

    #[test]
    fn checks_record_only_in_order() {
        let now = Utc::now();
        let mut c = AuthorityChecklist::new();
        // Skipping the first check is refused.
        assert!(matches!(
            c.record("tenant_lifecycle", now),
            Err(AuthorityError::OutOfOrder { .. })
        ));
        c.record("authenticated_principal", now).unwrap();
        c.record("tenant_context", now).unwrap();
        // Re-recording an earlier check is a no-op, not an error.
        c.record("tenant_context", now).unwrap();
        assert_eq!(c.recorded().len(), 2);
        // Unknown checks are refused.
        assert!(matches!(
            c.record("definitely_not_a_check", now),
            Err(AuthorityError::UnknownCheck(_))
        ));
    }

    #[test]
    fn finish_requires_every_check() {
        let s = scope();
        let mut c = AuthorityChecklist::new();
        let now = Utc::now();
        c.record("authenticated_principal", now).unwrap();
        let err = c.finish(&s, now).unwrap_err();
        assert!(matches!(err, AuthorityError::MissingChecks(missing) if missing.len() == 10));
    }

    #[test]
    fn authority_authorizes_only_its_scope() {
        let s = scope();
        let authority = complete_checklist().finish(&s, Utc::now()).unwrap();
        assert!(authority.authorizes(&s));
        let other = ExecutionScope::new(
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
            BotModule::Sniper,
            ExecutionMode::Paper,
        )
        .unwrap();
        assert!(!authority.authorizes(&other));
    }

    #[test]
    fn authority_is_serializable_for_audit() {
        let s = scope();
        let authority = complete_checklist().finish(&s, Utc::now()).unwrap();
        let json = serde_json::to_string(&authority).unwrap();
        assert!(json.contains("risk_permission"));
        let back: ExecutionAuthority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, authority);
    }
}
