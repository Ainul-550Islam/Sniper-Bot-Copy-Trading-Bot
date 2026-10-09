//! Configuration version and optimistic concurrency (STEP 3 file 32).
//!
//! Every stored tenant configuration carries a monotonically increasing
//! [`ConfigVersion`]. Updates compare-and-swap on it: two concurrent
//! editors cannot silently clobber each other, and the cache can tell a
//! stale copy from a fresh one without re-reading the row.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bot_core::tenant::OrganizationId;

/// A configuration document version (1, 2, 3, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigVersion(u64);

impl ConfigVersion {
    /// The first version of any document.
    pub fn first() -> Self {
        ConfigVersion(1)
    }

    /// Build from a raw counter (must be >= 1).
    pub fn from_raw(raw: u64) -> Option<Self> {
        (raw >= 1).then_some(ConfigVersion(raw))
    }

    /// The raw counter.
    pub fn raw(self) -> u64 {
        self.0
    }

    /// The next version.
    pub fn next(self) -> Self {
        ConfigVersion(self.0 + 1)
    }
}

impl std::fmt::Display for ConfigVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// One stored configuration document with its version metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigRecord {
    /// The tenant that owns the document.
    pub organization_id: OrganizationId,
    /// The document version.
    pub version: ConfigVersion,
    /// The typed configuration.
    pub config: super::model::TenantConfigModel,
    /// Who last changed it (user id, operator label or "system").
    pub updated_by: Option<String>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
}

impl ConfigRecord {
    /// A first version for a tenant.
    pub fn initial(
        organization_id: OrganizationId,
        config: super::model::TenantConfigModel,
        updated_by: Option<String>,
        now: DateTime<Utc>,
    ) -> Self {
        ConfigRecord {
            organization_id,
            version: ConfigVersion::first(),
            config,
            updated_by,
            updated_at: now,
        }
    }

    /// The version a writer must present to supersede this record.
    pub fn expected_version(&self) -> ConfigVersion {
        self.version
    }

    /// Derive the successor record (the store performs the CAS).
    pub fn successor(
        &self,
        config: super::model::TenantConfigModel,
        updated_by: Option<String>,
        now: DateTime<Utc>,
    ) -> ConfigRecord {
        ConfigRecord {
            organization_id: self.organization_id,
            version: self.version.next(),
            config,
            updated_by,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant_config::model::TenantConfigModel;

    #[test]
    fn versions_start_at_one_and_increase() {
        assert_eq!(ConfigVersion::first().raw(), 1);
        assert_eq!(ConfigVersion::from_raw(0), None);
        assert_eq!(ConfigVersion::from_raw(9).unwrap().next().raw(), 10);
        assert!(ConfigVersion::first() < ConfigVersion::first().next());
        assert_eq!(ConfigVersion::first().to_string(), "v1");
    }

    #[test]
    fn records_carry_cas_metadata() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let first = ConfigRecord::initial(
            org,
            TenantConfigModel::default(),
            Some("op".to_string()),
            now,
        );
        assert_eq!(first.expected_version(), ConfigVersion::first());
        let second = first.successor(TenantConfigModel::default(), Some("op-2".to_string()), now);
        assert_eq!(second.version.raw(), 2);
        assert_eq!(second.organization_id, org);
        assert_eq!(second.expected_version().raw(), 2);
    }
}
