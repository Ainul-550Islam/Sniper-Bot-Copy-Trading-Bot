//! Aggregate runtime dependency health (Batch 4).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    Degraded,
    Unavailable,
    NotConfigured,
}

impl Health {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::NotConfigured => "not_configured",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub health: Health,
    pub required: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyHealthReport {
    pub dependencies: Vec<Dependency>,
    pub overall: Health,
}

pub fn aggregate(deps: Vec<Dependency>) -> DependencyHealthReport {
    // overall: required Unavailable => Unavailable, any Degraded => Degraded, else Healthy (NotConfigured for optional is not Degraded)
    let mut overall = Health::Healthy;
    for d in &deps {
        match (&d.health, d.required) {
            (Health::Unavailable, true) => {
                overall = Health::Unavailable;
                break;
            }
            (Health::Degraded, _) => {
                if overall != Health::Unavailable {
                    overall = Health::Degraded;
                }
            }
            (Health::Unavailable, false) if overall == Health::Healthy => {
                overall = Health::Degraded;
            }
            _ => {}
        }
    }
    DependencyHealthReport {
        dependencies: deps,
        overall,
    }
}

pub fn dependency(name: &str, health: Health, required: bool, detail: &str) -> Dependency {
    let safe = if detail.to_ascii_lowercase().contains("secret")
        || detail.to_ascii_lowercase().contains("password")
    {
        "<redacted>"
    } else {
        detail
    };
    Dependency {
        name: name.to_string(),
        health,
        required,
        detail: safe.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_unavailable_is_unavailable() {
        let r = aggregate(vec![
            dependency("postgres", Health::Unavailable, true, "down"),
            dependency("redis", Health::Healthy, false, "ok"),
        ]);
        assert_eq!(r.overall, Health::Unavailable);
    }
    #[test]
    fn optional_unavailable_is_degraded() {
        let r = aggregate(vec![
            dependency("postgres", Health::Healthy, true, "ok"),
            dependency("redis", Health::Unavailable, false, "down"),
        ]);
        assert_eq!(r.overall, Health::Degraded);
    }
    #[test]
    fn all_healthy_is_healthy() {
        let r = aggregate(vec![
            dependency("postgres", Health::Healthy, true, "ok"),
            dependency("telegram", Health::NotConfigured, false, "not set"),
        ]);
        assert_eq!(r.overall, Health::Healthy);
    }
    #[test]
    fn degraded_propagates() {
        let r = aggregate(vec![
            dependency("rpc", Health::Degraded, true, "lag"),
            dependency("postgres", Health::Healthy, true, "ok"),
        ]);
        assert_eq!(r.overall, Health::Degraded);
    }
    #[test]
    fn redacts() {
        let d = dependency("postgres", Health::Unavailable, true, "password=secret");
        assert_eq!(d.detail, "<redacted>");
    }
}
