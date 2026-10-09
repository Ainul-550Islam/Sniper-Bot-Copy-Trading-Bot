//! Real service-dependency verification helpers (Batch 4).
//! Check PostgreSQL, Redis, migration state, required tables, basic connectivity.
//! Never expose connection strings or credentials. Distinguish unavailable dependency from application failure.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Pass,
    Fail,
    NotConfigured,
    Unavailable,
}

impl ServiceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotConfigured => "not_configured",
            Self::Unavailable => "unavailable",
        }
    }
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCheck {
    pub service: String,
    pub status: ServiceStatus,
    pub detail: String,
    pub required: bool,
}

impl ServiceCheck {
    pub fn new(
        service: impl Into<String>,
        status: ServiceStatus,
        detail: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            service: service.into(),
            status,
            detail: detail.into(),
            required,
        }
    }
    pub fn pass(service: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(service, ServiceStatus::Pass, detail, true)
    }
    pub fn redacted_detail(&self) -> String {
        // Never leak url/credential fragments
        let d = self.detail.to_ascii_lowercase();
        if d.contains("postgres://")
            || d.contains("redis://")
            || d.contains("password")
            || d.contains("secret")
        {
            "<redacted>".to_string()
        } else {
            self.detail.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationServicesReport {
    pub postgres: ServiceCheck,
    pub redis: ServiceCheck,
    pub migrations: ServiceCheck,
    pub tables: ServiceCheck,
    pub overall: ServiceStatus,
}

pub fn evaluate_services(
    pg_url_present: bool,
    pg_reachable: Option<bool>,
    redis_url_present: bool,
    redis_reachable: Option<bool>,
    migration_high_water: Option<u32>,
    expected_high_water: u32,
    required_tables_present: Option<bool>,
) -> IntegrationServicesReport {
    let postgres = match (pg_url_present, pg_reachable) {
        (false, _) => ServiceCheck::new(
            "postgres",
            ServiceStatus::NotConfigured,
            "DATABASE_URL not set",
            true,
        ),
        (true, None) => ServiceCheck::new(
            "postgres",
            ServiceStatus::Unavailable,
            "postgres url set but connectivity not tested",
            true,
        ),
        (true, Some(true)) => ServiceCheck::pass("postgres", "reachable"),
        (true, Some(false)) => ServiceCheck::new(
            "postgres",
            ServiceStatus::Unavailable,
            "postgres unreachable",
            true,
        ),
    };
    let redis = match (redis_url_present, redis_reachable) {
        (false, _) => ServiceCheck::new(
            "redis",
            ServiceStatus::NotConfigured,
            "REDIS_URL not set",
            false,
        ),
        (true, None) => ServiceCheck::new(
            "redis",
            ServiceStatus::Unavailable,
            "redis url set but not tested",
            false,
        ),
        (true, Some(true)) => ServiceCheck::new("redis", ServiceStatus::Pass, "reachable", false),
        (true, Some(false)) => ServiceCheck::new(
            "redis",
            ServiceStatus::Unavailable,
            "redis unreachable",
            false,
        ),
    };
    let migrations = match migration_high_water {
        None => ServiceCheck::new(
            "migrations",
            ServiceStatus::Fail,
            "migration state unknown",
            true,
        ),
        Some(v) if v == expected_high_water => {
            ServiceCheck::pass("migrations", format!("high_water {v:04}"))
        }
        Some(v) if v < expected_high_water => ServiceCheck::new(
            "migrations",
            ServiceStatus::Fail,
            format!("high_water {v:04} expected {expected_high_water:04}"),
            true,
        ),
        Some(v) => ServiceCheck::new(
            "migrations",
            ServiceStatus::Fail,
            format!("high_water {v:04} unexpected"),
            true,
        ),
    };
    let tables = match required_tables_present {
        None => ServiceCheck::new(
            "tables",
            ServiceStatus::Fail,
            "table check not executed",
            true,
        ),
        Some(true) => ServiceCheck::pass("tables", "required tables present"),
        Some(false) => ServiceCheck::new(
            "tables",
            ServiceStatus::Fail,
            "missing required tables",
            true,
        ),
    };

    let overall = if postgres.status == ServiceStatus::Fail
        || migrations.status == ServiceStatus::Fail
        || tables.status == ServiceStatus::Fail
    {
        ServiceStatus::Fail
    } else if postgres.status == ServiceStatus::Unavailable {
        ServiceStatus::Unavailable
    } else if postgres.status == ServiceStatus::Pass
        && migrations.status.is_pass()
        && tables.status.is_pass()
    {
        ServiceStatus::Pass
    } else {
        ServiceStatus::Fail
    };

    IntegrationServicesReport {
        postgres,
        redis,
        migrations,
        tables,
        overall,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_not_configured_distinction() {
        let r = evaluate_services(false, None, false, None, Some(21), 21, Some(true));
        assert_eq!(r.postgres.status, ServiceStatus::NotConfigured);
        assert_eq!(r.overall, ServiceStatus::Fail);
    }
    #[test]
    fn all_pass() {
        let r = evaluate_services(true, Some(true), true, Some(true), Some(21), 21, Some(true));
        assert_eq!(r.overall, ServiceStatus::Pass);
        assert_eq!(r.migrations.status, ServiceStatus::Pass);
    }
    #[test]
    fn migration_mismatch_fail() {
        let r = evaluate_services(true, Some(true), false, None, Some(20), 21, Some(true));
        assert_eq!(r.migrations.status, ServiceStatus::Fail);
        assert_eq!(r.overall, ServiceStatus::Fail);
    }
    #[test]
    fn never_leaks_url() {
        let c = ServiceCheck::new(
            "postgres",
            ServiceStatus::Unavailable,
            "postgres://user:secret@host/db",
            true,
        );
        assert_eq!(c.redacted_detail(), "<redacted>");
    }
    #[test]
    fn optional_redis_unavailable_not_fail_overall_if_pg_pass() {
        let r = evaluate_services(
            true,
            Some(true),
            true,
            Some(false),
            Some(21),
            21,
            Some(true),
        );
        // overall still Pass because redis is optional; postgres pass + migrations pass
        assert_eq!(r.redis.status, ServiceStatus::Unavailable);
        assert_eq!(r.overall, ServiceStatus::Pass);
    }
}
