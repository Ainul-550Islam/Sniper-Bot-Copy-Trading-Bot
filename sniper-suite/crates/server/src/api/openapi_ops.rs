//! OpenAPI schemas for operator/release/deployment/readiness evidence (Batch 4). No secrets.

use serde_json::{json, Value};

pub fn schemas() -> Value {
    json!({
        "DeploymentPreflight": {
            "type":"object",
            "required":["mode","overall","checks"],
            "properties":{
                "mode":{"type":"string"},
                "overall":{"type":"string","enum":["pass","warn","block"]},
                "checks":{"type":"array","items":{"type":"object","required":["name","status","detail"],"properties":{"name":{"type":"string"},"status":{"type":"string","enum":["pass","warn","block"]},"detail":{"type":"string"}}}}
            }
        },
        "RuntimeConfigReport": {
            "type":"object",
            "required":["environment","categories"],
            "properties":{
                "environment":{"type":"string"},
                "categories":{"type":"array","items":{"type":"object"}},
                "migration_high_water":{"type":["string","null"]}
            }
        },
        "DependencyHealth": {
            "type":"object",
            "required":["dependencies","overall"],
            "properties":{
                "dependencies":{"type":"array","items":{"type":"object"}},
                "overall":{"type":"string","enum":["healthy","degraded","unavailable","not_configured"]}
            }
        },
        "ReleaseArtifact": {
            "type":"object",
            "required":["version","filename","sha256","size_bytes","target"],
            "properties":{
                "version":{"type":"string"},
                "filename":{"type":"string"},
                "sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},
                "size_bytes":{"type":"integer"},
                "target":{"type":"string"}
            }
        },
        "SecurityEvidence": {
            "type":"object",
            "required":["checks","overall"],
            "properties":{
                "checks":{"type":"array","items":{"type":"object"}},
                "overall":{"type":"string","enum":["pass","fail","not_run","warn"]}
            }
        },
        "SecuritySummary": {
            "type":"object",
            "required":["organization_id","websocket_auth","audit_available"],
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "websocket_auth":{"type":"string"},
                "audit_available":{"type":"boolean"}
            }
        },
        "BackupStatus": {
            "type":"object",
            "required":["organization_id","retention_configured","protection"],
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "retention_configured":{"type":"boolean"},
                "last_verified_restore":{"type":["string","null"]}
            }
        }
    })
}

pub fn paths() -> Value {
    json!({
        "/api/ops/preflight": {"get": {"operationId":"ops.preflight","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"preflight"},"401":{"$ref":"#/components/responses/Unauthorized"},"403":{"$ref":"#/components/responses/Forbidden"},"503":{"description":"service unavailable"}}}},
        "/api/ops/runtime-config": {"get": {"operationId":"ops.runtimeConfig","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"runtime config"}}}},
        "/api/ops/dependency-health": {"get": {"operationId":"ops.dependencyHealth","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"health"}}}},
        "/api/ops/release-artifact": {"get": {"operationId":"ops.releaseArtifact","responses":{"200":{"description":"artifact"}}}},
        "/api/saas/security/summary": {"get": {"operationId":"saas.securitySummary","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"security summary"},"401":{"$ref":"#/components/responses/Unauthorized"},"403":{"$ref":"#/components/responses/Forbidden"},"404":{"description":"not found"}}}},
        "/api/saas/backup/status": {"get": {"operationId":"saas.backupStatus","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"backup status"}}}}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_no_secrets() {
        let s = schemas().to_string().to_ascii_lowercase();
        for banned in ["secret", "password", "private_key"] {
            assert!(!s.contains(banned), "banned {banned}");
        }
    }
    #[test]
    fn paths_document_auth() {
        let p = paths().to_string();
        assert!(p.contains("401"));
        assert!(p.contains("403"));
    }
}
