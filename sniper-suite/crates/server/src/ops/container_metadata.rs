//! Runtime/container metadata model (Batch 5).
//! Version, build id, platform, runtime version, migration high-water, startup ts. No secrets.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerMetadata {
    pub application_version: String,
    pub build_identifier: String,
    pub target_platform: String,
    pub rust_version: String,
    pub migration_high_water: String,
    pub startup_timestamp: String,
}

impl ContainerMetadata {
    pub fn new(
        application_version: impl Into<String>,
        build_identifier: impl Into<String>,
        target_platform: impl Into<String>,
        rust_version: impl Into<String>,
        migration_high_water: impl Into<String>,
        startup_timestamp: impl Into<String>,
    ) -> Self {
        Self {
            application_version: application_version.into(),
            build_identifier: build_identifier.into(),
            target_platform: target_platform.into(),
            rust_version: rust_version.into(),
            migration_high_water: migration_high_water.into(),
            startup_timestamp: startup_timestamp.into(),
        }
    }

    pub fn from_env() -> Self {
        Self {
            application_version: env!("CARGO_PKG_VERSION").into(),
            build_identifier: option_env!("BUILD_IDENTIFIER").unwrap_or("dev").into(),
            target_platform: std::env::consts::ARCH.into(),
            rust_version: option_env!("RUSTC_VERSION").unwrap_or("1.82").into(),
            migration_high_water: "0021".into(),
            startup_timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.application_version.trim().is_empty() {
            return Err("application_version required".into());
        }
        if self.build_identifier.trim().is_empty() {
            return Err("build_identifier required".into());
        }
        if self.migration_high_water.trim().is_empty() {
            return Err("migration_high_water required".into());
        }
        Ok(())
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "application_version": self.application_version,
            "build_identifier": self.build_identifier,
            "target_platform": self.target_platform,
            "rust_version": self.rust_version,
            "migration_high_water": self.migration_high_water,
            "startup_timestamp": self.startup_timestamp
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_json_has_no_secrets() {
        let m = ContainerMetadata::new(
            "0.1.0",
            "abc123",
            "x86_64",
            "1.82",
            "0021",
            "2026-09-24T00:00:00Z",
        );
        let j = m.to_safe_json().to_string().to_ascii_lowercase();
        assert!(!j.contains("password"));
        assert!(!j.contains("secret"));
        assert!(!j.contains("token"));
        assert!(j.contains("0.1.0"));
    }

    #[test]
    fn validation_requires_version() {
        let mut m = ContainerMetadata::new("0.1.0", "id", "x86_64", "1.82", "0021", "now");
        assert!(m.validate().is_ok());
        m.application_version = "  ".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn from_env_has_version() {
        let m = ContainerMetadata::from_env();
        assert!(!m.application_version.is_empty());
        assert!(!m.migration_high_water.is_empty());
    }
}
