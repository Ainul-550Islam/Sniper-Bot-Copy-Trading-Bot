//! A small, reusable health / readiness registry.
//!
//! Liveness and readiness are deliberately separate:
//!
//! * **Liveness** (`/health`) answers "is the process up?" and must never
//!   depend on external services — otherwise a downstream outage would make
//!   the orchestrator restart a process that is fine.
//! * **Readiness** (`/ready`) answers "can this instance do its job?" and
//!   aggregates component states (module loops heartbeating, RPC reachable).
//!
//! Components are updated by a sampler (see the server's `obs` module); the
//! HTTP handlers only read snapshots.
//!
//! SECURITY: `detail` strings are rendered into HTTP responses. Callers must
//! only put non-sensitive facts in them (booleans, counts, enum names) —
//! never error payloads, URLs, keys or wallet material. RPC error strings can
//! embed provider URLs, which may contain API keys.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::Instant;

use serde::Serialize;

/// One component's health/readiness state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentStatus {
    /// True when the component is functioning normally.
    pub healthy: bool,
    /// True when the component is ready to serve its role.
    pub ready: bool,
    /// Human-readable, **non-sensitive** status detail (see module docs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ComponentStatus {
    /// A component that is both healthy and ready.
    pub fn ok(detail: impl Into<String>) -> Self {
        ComponentStatus {
            healthy: true,
            ready: true,
            detail: Some(detail.into()),
        }
    }

    /// A component that is not ready (and reports why, safely).
    pub fn not_ready(detail: impl Into<String>) -> Self {
        ComponentStatus {
            healthy: false,
            ready: false,
            detail: Some(detail.into()),
        }
    }
}

/// One component line in a [`HealthReport`].
#[derive(Debug, Clone, Serialize)]
pub struct ComponentReport {
    /// Stable component name (e.g. `"rpc"`, `"sniper"`).
    pub name: String,
    /// The component's state.
    #[serde(flatten)]
    pub status: ComponentStatus,
}

/// Aggregated snapshot of every registered component.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    /// `"ok"` when every component is healthy, `"degraded"` otherwise.
    pub status: &'static str,
    /// True when every component reports ready.
    pub ready: bool,
    /// True when every component reports healthy.
    pub healthy: bool,
    /// Seconds since the registry (process) started.
    pub uptime_secs: u64,
    /// Per-component states, sorted by name.
    pub components: Vec<ComponentReport>,
}

/// Thread-safe registry of component health, created once at startup.
pub struct HealthRegistry {
    components: RwLock<BTreeMap<String, ComponentStatus>>,
    started: Instant,
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthRegistry {
    /// An empty registry whose clock starts now.
    pub fn new() -> Self {
        HealthRegistry {
            components: RwLock::new(BTreeMap::new()),
            started: Instant::now(),
        }
    }

    /// Insert or update a component's status.
    pub fn set(&self, name: &str, status: ComponentStatus) {
        let mut map = self.write();
        map.insert(name.to_string(), status);
    }

    /// Remove a component (e.g. a module that was disabled and torn down).
    pub fn remove(&self, name: &str) {
        let mut map = self.write();
        map.remove(name);
    }

    /// Names of all registered components, sorted.
    pub fn component_names(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// Overall readiness: every registered component is ready. An empty
    /// registry counts as ready (nothing is being waited for); the server
    /// registers its components at startup, so this window is momentary.
    pub fn ready(&self) -> bool {
        self.read().values().all(|c| c.ready)
    }

    /// Overall health: every registered component is healthy.
    pub fn healthy(&self) -> bool {
        self.read().values().all(|c| c.healthy)
    }

    /// Process uptime as observed by this registry.
    pub fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// A serialisable snapshot for the `/ready` response body.
    pub fn snapshot(&self) -> HealthReport {
        let map = self.read();
        let components = map
            .iter()
            .map(|(name, status)| ComponentReport {
                name: name.clone(),
                status: status.clone(),
            })
            .collect::<Vec<_>>();
        let healthy = components.iter().all(|c| c.status.healthy);
        let ready = components.iter().all(|c| c.status.ready);
        HealthReport {
            status: if healthy { "ok" } else { "degraded" },
            ready,
            healthy,
            uptime_secs: self.started.elapsed().as_secs(),
            components,
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, ComponentStatus>> {
        self.components.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, ComponentStatus>> {
        self.components.write().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_is_ready_and_ok() {
        let reg = HealthRegistry::new();
        assert!(reg.ready());
        assert!(reg.healthy());
        let snap = reg.snapshot();
        assert_eq!(snap.status, "ok");
        assert!(snap.ready);
        assert!(snap.components.is_empty());
    }

    #[test]
    fn one_unready_component_fails_readiness_but_not_liveness_reporting() {
        let reg = HealthRegistry::new();
        reg.set("api", ComponentStatus::ok("serving"));
        reg.set("rpc", ComponentStatus::not_ready("consecutive_failures=3"));
        assert!(!reg.ready(), "any unready component fails readiness");
        assert!(!reg.healthy());
        let snap = reg.snapshot();
        assert_eq!(snap.status, "degraded");
        assert!(!snap.ready);
        // Components sort by name.
        let names: Vec<&str> = snap.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["api", "rpc"]);
        assert!(snap.components[0].status.ready);
        assert!(!snap.components[1].status.ready);
        assert_eq!(
            snap.components[1].status.detail.as_deref(),
            Some("consecutive_failures=3")
        );
    }

    #[test]
    fn unhealthy_but_ready_is_degraded_yet_ready() {
        // A module can be serving (ready) while degraded (consecutive errors).
        let reg = HealthRegistry::new();
        reg.set(
            "sniper",
            ComponentStatus {
                healthy: false,
                ready: true,
                detail: Some("errors=2".into()),
            },
        );
        assert!(reg.ready());
        assert!(!reg.healthy());
        assert_eq!(reg.snapshot().status, "degraded");
    }

    #[test]
    fn set_overwrites_and_remove_drops() {
        let reg = HealthRegistry::new();
        reg.set("rpc", ComponentStatus::not_ready("down"));
        assert!(!reg.ready());
        reg.set("rpc", ComponentStatus::ok("up"));
        assert!(reg.ready());
        reg.remove("rpc");
        assert_eq!(reg.component_names(), Vec::<String>::new());
        assert!(reg.ready(), "empty again => ready");
    }

    #[test]
    fn snapshot_serialises_to_safe_json() {
        let reg = HealthRegistry::new();
        reg.set("rpc", ComponentStatus::ok("consecutive_failures=0"));
        let v = serde_json::to_value(reg.snapshot()).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["ready"], true);
        assert_eq!(v["components"][0]["name"], "rpc");
        assert_eq!(v["components"][0]["healthy"], true);
        assert_eq!(v["components"][0]["detail"], "consecutive_failures=0");
        // Uptime is a number, never negative.
        assert!(v["uptime_secs"].as_u64().is_some());
    }

    #[test]
    fn concurrent_updates_are_consistent() {
        use std::sync::Arc;
        let reg = Arc::new(HealthRegistry::new());
        let mut handles = Vec::new();
        for i in 0..8 {
            let reg = Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                for j in 0..100 {
                    reg.set(&format!("c{i}"), ComponentStatus::ok(format!("{j}")));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(reg.component_names().len(), 8);
        assert!(reg.ready());
    }
}
