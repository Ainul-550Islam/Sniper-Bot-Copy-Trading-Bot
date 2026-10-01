//! Stable tenant-aware job identity (tenant-isolation file 54).
//!
//! [`JobIdentity`] wraps the scheduler's [`JobKey`] (tenant + module +
//! name) with the STABLE identity forms the rest of the system needs:
//! a human-readable identity string for logs and a deduplication key
//! that survives process restarts (recovery dedup, job-claim
//! correlation). Two jobs of different tenants NEVER share an
//! identity — the tenant is part of every form.

use bot_core::tenant::{ModuleKind, OrganizationId};

use super::jobs::{JobKey, TenantJob};

/// A job's stable, tenant-scoped identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobIdentity {
    organization_id: OrganizationId,
    module: ModuleKind,
    name: &'static str,
}

impl JobIdentity {
    /// Build from parts.
    pub fn new(organization_id: OrganizationId, module: ModuleKind, name: &'static str) -> Self {
        JobIdentity {
            organization_id,
            module,
            name,
        }
    }

    /// Adopt a scheduler key.
    pub fn from_key(key: &JobKey) -> Self {
        JobIdentity {
            organization_id: key.organization_id,
            module: key.module,
            name: key.name,
        }
    }

    /// A job's identity from its own description.
    pub fn from_job(job: &dyn TenantJob) -> Self {
        Self::from_key(&job.key())
    }

    /// The scheduler key form.
    pub fn key(&self) -> JobKey {
        JobKey::new(self.organization_id, self.module, self.name)
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The module the job serves.
    pub fn module(&self) -> ModuleKind {
        self.module
    }

    /// The job's stable name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Human-readable identity for logs and diagnostics:
    /// `job:<organization>:<module>:<name>` (tenant-qualified).
    pub fn identity_string(&self) -> String {
        format!("job:{}:{}:{}", self.organization_id, self.module, self.name)
    }

    /// The stable deduplication/recovery key: identical across process
    /// restarts, unique per (tenant, module, name) — safe as a
    /// job-claim correlation or recovery ledger id.
    pub fn dedup_key(&self) -> String {
        // Deterministic prefix + tenant + module + name; no timestamps,
        // no process ids — restarts produce the SAME key.
        format!(
            "job-dedup:{}:{}:{}",
            self.organization_id, self.module, self.name
        )
    }

    /// Does this identity belong to this tenant?
    pub fn belongs_to(&self, organization_id: OrganizationId) -> bool {
        self.organization_id == organization_id
    }
}

impl std::fmt::Display for JobIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.identity_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;
    use std::collections::HashSet;

    #[test]
    fn identity_round_trips_through_the_scheduler_key() {
        let org = OrganizationId::new();
        let identity = JobIdentity::new(org, ModuleKind::Sniper, "sweep");
        let key = identity.key();
        assert_eq!(JobIdentity::from_key(&key), identity);
        assert_eq!(key.organization_id, org);
        assert_eq!(key.module, ModuleKind::Sniper);
        assert_eq!(key.name, "sweep");
    }

    #[test]
    fn two_tenants_never_share_an_identity() {
        let a = JobIdentity::new(OrganizationId::new(), ModuleKind::Copy, "recon");
        let b = JobIdentity::new(OrganizationId::new(), ModuleKind::Copy, "recon");
        assert_ne!(a.identity_string(), b.identity_string());
        assert_ne!(a.dedup_key(), b.dedup_key());
        assert!(!a.belongs_to(b.organization_id()));
        // The same job for the same tenant IS the same identity.
        let a2 = JobIdentity::new(a.organization_id(), ModuleKind::Copy, "recon");
        assert_eq!(a.dedup_key(), a2.dedup_key());
    }

    #[test]
    fn the_dedup_key_is_stable_and_identity_string_is_readable() {
        let org = OrganizationId::new();
        let identity = JobIdentity::new(org, ModuleKind::Copy, "recon");
        assert_eq!(identity.identity_string(), format!("job:{org}:copy:recon"));
        assert_eq!(identity.dedup_key(), format!("job-dedup:{org}:copy:recon"));
        // Restart-stable: rebuilding produces the same key.
        let rebuilt = JobIdentity::new(org, ModuleKind::Copy, "recon");
        assert_eq!(identity.dedup_key(), rebuilt.dedup_key());
        // Distinct modules of one tenant are distinct identities.
        let other = JobIdentity::new(org, ModuleKind::Sniper, "recon");
        let mut set = HashSet::new();
        set.insert(identity.dedup_key());
        set.insert(other.dedup_key());
        assert_eq!(set.len(), 2);
    }
}
