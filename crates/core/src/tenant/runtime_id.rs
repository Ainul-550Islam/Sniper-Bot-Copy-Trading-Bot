//! Strong runtime identifier (STEP 3 file 03).
//!
//! A [`RuntimeId`] names ONE runtime instance of ONE tenant: the process
//! (or process partition) that is currently allowed to execute that
//! tenant's trades. It is a UUID newtype over the same primitives as
//! [`super::model::OrganizationId`] — a function that takes a
//! `RuntimeId` cannot be handed an organization id, a worker name or a
//! session id by accident.
//!
//! The id is metadata only: it never authorizes anything by itself.
//! Authorization is the fence check in
//! `crates/server/src/runtime_registry/fencing.rs` (runtime id + generation
//! must both match the registry's current active runtime).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::TenantIdError;

/// One tenant runtime instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeId(pub Uuid);

impl RuntimeId {
    /// A fresh runtime identifier (used when a runtime registers).
    pub fn new() -> Self {
        RuntimeId(Uuid::new_v4())
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parse from the canonical string form.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(RuntimeId)
    }

    /// The nil runtime id — the "uninitialized" value. Never a valid
    /// runtime: registration always mints a real id.
    pub fn is_nil(&self) -> bool {
        self.0 == Uuid::nil()
    }
}

impl Default for RuntimeId {
    fn default() -> Self {
        RuntimeId::new()
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for RuntimeId {
    fn from(u: Uuid) -> Self {
        RuntimeId(u)
    }
}

impl From<RuntimeId> for Uuid {
    fn from(r: RuntimeId) -> Uuid {
        r.0
    }
}

impl FromStr for RuntimeId {
    type Err = TenantIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RuntimeId::parse(s).ok_or_else(|| TenantIdError {
            kind: "RuntimeId",
            value: s.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trip() {
        let r = RuntimeId::new();
        let s = r.to_string();
        assert_eq!(RuntimeId::parse(&s), Some(r));
        assert_eq!(s.parse::<RuntimeId>().unwrap(), r);
        assert!("not-a-uuid".parse::<RuntimeId>().is_err());
    }

    #[test]
    fn fresh_ids_are_distinct_and_never_nil() {
        let a = RuntimeId::new();
        let b = RuntimeId::new();
        assert_ne!(a, b);
        assert!(!a.is_nil());
    }

    #[test]
    fn uuid_conversions() {
        let u = Uuid::new_v4();
        let r = RuntimeId::from(u);
        assert_eq!(r.as_uuid(), u);
        assert_eq!(Uuid::from(r), u);
    }

    #[test]
    fn ordering_is_stable() {
        let mut v = [RuntimeId::new(), RuntimeId::new(), RuntimeId::new()];
        v.sort();
        assert!(v[0] <= v[1] && v[1] <= v[2]);
    }
}
