//! OpenAPI schemas for operator/release/deployment/readiness evidence (Batch 4). No secrets.

use serde_json::{json, Value};

pub fn schemas() -> Value {
    json!({
        "MigrationHealth": {
            "type":"object",
            "required":["state","healthy","embedded_count","applied_count","summary"],
            "properties":{
                "state":{"type":"string","enum":["in_sync","ahead","pending","dirty","checksum_mismatch"],
                    "description":"`ahead` (database newer than the binary) is HEALTHY — that is the normal state during a rollback."},
                "healthy":{"type":"boolean"},
                "binary_high_water":{"type":["integer","null"]},
                "database_high_water":{"type":["integer","null"]},
                "embedded_count":{"type":"integer"},
                "applied_count":{"type":"integer"},
                "pending_versions":{"type":"array","items":{"type":"integer"},"description":"Required by this build, not applied. Queries using them fail at request time."},
                "unknown_versions":{"type":"array","items":{"type":"integer"},"description":"Applied but unknown to this build."},
                "checksum_mismatches":{"type":"array","items":{"type":"integer"},"description":"Same version, different content. Never automatic to resolve."},
                "failed_versions":{"type":"array","items":{"type":"integer"}},
                "versions":{"type":"array","items":{"type":"object"}},
                "summary":{"type":"string","description":"One actionable sentence. Never contains a connection string or checksum bytes."}
            }
        },





        "SecuritySummary": {
            "type":"object",
            "required":["organization_id","mfa_status","session_status","api_key_status","websocket_auth","custody_mode","audit_available","lifecycle_state","as_of"],
            "description":"Every field is derived from this deployment's configuration or from the credential that made the request. None of them is a constant.",
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "mfa_status":{"type":"string","enum":["not_supported"],
                    "description":"`not_supported` in every build that has no MFA implementation — stated plainly rather than as `unknown`, which would imply the capability might exist."},
                "session_status":{"type":"string","enum":["session","api_key"],"description":"How THIS request authenticated."},
                "api_key_status":{"type":"string","enum":["in_use","not_used_for_this_request"]},
                "websocket_auth":{"type":"string"},
                "custody_mode":{"type":"string","enum":["local","vault","kms","hsm"],
                    "description":"Read from [signing].provider. Only `local` is implemented; the others fail startup rather than falling back."},
                "audit_available":{"type":"boolean","description":"True only when the audit trail is DB-chained and therefore verifiable after a restart."},
                "lifecycle_state":{"type":"string"},
                "as_of":{"type":"string","format":"date-time"}
            }
        },
        "BackupStatus": {
            "type":"object",
            "required":["organization_id","state","protected","offsite","pitr","rpo_basis","summary","as_of"],
            "description":"Reports ONLY what the append-only backup ledger proves. A deployment with no ledger answers state=not_configured; this endpoint never asserts protection it cannot evidence.",
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "state":{"type":"string","enum":["verified","unverified","stale","failing","not_configured"],
                    "description":"verified = a current backup AND a restore drill inside the quarterly window. unverified = backups run but no restore has ever been proven — an untested backup is an assumption, so this is NOT protected."},
                "protected":{"type":"boolean","description":"True only for state=verified."},
                "last_backup_at":{"type":["string","null"],"format":"date-time"},
                "last_verified_restore":{"type":["string","null"],"format":"date-time","description":"null = no restore has ever been verified."},
                "retention_days":{"type":["integer","null"]},
                "encrypted":{"type":["boolean","null"],"description":"Whether every recorded backup was encrypted at rest. null = nothing recorded."},
                "backup_count":{"type":"integer"},
                "offsite":{"type":"object","required":["state","configured","includes_pitr"],
                    "description":"Geographic redundancy, a SEPARATE axis from `protected`. `protected` answers \"can this be restored, proven by drill\"; this answers \"would a copy survive losing this host\".",
                    "properties":{
                        "state":{"type":"string","enum":["not_configured","current","stale","failing"]},
                        "configured":{"type":"boolean"},
                        "last_sync_at":{"type":["string","null"],"format":"date-time"},
                        "includes_pitr":{"type":"boolean","description":"Whether the last successful sync also carried the base backups and WAL archive. False with PITR configured means point-in-time recovery does not survive losing this host."}
                    }},
                "pitr":{"type":"object","required":["state"],
                    "description":"Point-in-time recovery — the axis that decides how MUCH data a disaster costs, as opposed to whether anything can be recovered at all.",
                    "properties":{
                        "state":{"type":"string","enum":["not_configured","failing","stale","unverified","current"],
                            "description":"not_configured = no WAL archiving, so the recovery point is the dump interval. unverified = archiving runs but no point-in-time recovery has ever been proven."},
                        "last_wal_archive_at":{"type":["string","null"],"format":"date-time"},
                        "last_base_backup_at":{"type":["string","null"],"format":"date-time"},
                        "last_verified_pitr":{"type":["string","null"],"format":"date-time"}
                    }},
                "rpo_estimate_seconds":{"type":["integer","null"],
                    "description":"Worst-case data loss in seconds as evidenced by the ledger. null when nothing is recorded — an unknown RPO is reported as unknown, never as zero."},
                "rpo_basis":{"type":"string","enum":["wal_archive","last_backup","unknown"],
                    "description":"Which record the estimate came from. A failing or stale WAL archive is never used as the basis."},
                "summary":{"type":"string","description":"One actionable sentence. Never contains a path, bucket, host or backup id."},
                "as_of":{"type":"string","format":"date-time"}
            }
        }
    })
}

/// Schemas for the surfaces that are described but not served.
///
/// They are kept out of the published `components.schemas` for the same
/// reason their paths are kept out of `paths()`: a schema no operation
/// can return is dead weight in every generated SDK, and it reads as a
/// capability the deployment does not have. Move one back into
/// `schemas()` in the same change that routes its endpoint.
pub fn unrouted_schemas_pending_implementation() -> Value {
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
        }
    })
}

pub fn paths() -> Value {
    // PUBLISHED = ROUTED. Every path below is served by
    // `crate::api::ops_routes` or `crate::saas`, and
    // `crates/server/tests/openapi_router_conformance.rs` fails the
    // build if that stops being true.
    //
    // `/api/ops/preflight`, `/api/ops/runtime-config` and
    // `/api/ops/release-artifact` were previously declared here and were
    // served by NOTHING: the document promised endpoints that answer
    // 404. Their builders still exist in `crate::ops`
    // (`deployment_preflight`, `runtime_config_report`,
    // `release_artifact`); they are not published until a router serves
    // them. See `unrouted_paths_pending_implementation` below.
    let ok = |description: &str, schema: &str| {
        json!({ "description": description, "content": { "application/json": {
            "schema": { "$ref": format!("#/components/schemas/{schema}") } } } })
    };
    let unauthorized = json!({ "$ref": "#/components/responses/Unauthorized" });
    let forbidden = json!({ "$ref": "#/components/responses/Forbidden" });

    json!({
        "/api/ops/migration-health": { "get": {
            "operationId": "ops.migrationHealth",
            "summary": "Does the database schema match this build? 200 for in_sync or ahead, 503 for pending, dirty or checksum_mismatch.",
            "description": "Platform administrators only: the answer describes the deployment, not a tenant, \
                and it exposes the shape of the schema. Production runs with DATABASE_AUTO_MIGRATE=false, \
                so a binary can start, pass /health and /ready, and still be missing the columns its \
                queries use — this endpoint is the only external way to see that. A database AHEAD of the \
                binary is reported as healthy because that is the expected state during a rollback.",
            "tags": ["ops"],
            "security": [{"bearerAuth": []}],
            "responses": {
                "200": ok("Schema agrees with this build (in_sync), or the database is ahead of it.", "MigrationHealth"),
                "401": unauthorized,
                "403": forbidden,
                "503": json!({ "description": "Migrations are pending, a migration did not complete, a checksum disagrees, no database is attached, or the migration table could not be read. The body carries `state` and `summary`.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/MigrationHealth" } } } }),
            },
        }},
        "/api/saas/security/summary": { "get": {
            "operationId": "saas.securitySummary",
            "summary": "Security posture for the caller's own tenant.",
            "tags": ["security"],
            "security": [{"bearerAuth": []}],
            "responses": {
                "200": ok("Security summary.", "SecuritySummary"),
                "401": unauthorized,
                "403": forbidden,
                "404": json!({ "description": "No such tenant, or not visible to this credential (404 rather than 403 to avoid an existence oracle)." }),
            },
        }},
        "/api/saas/backup/status": { "get": {
            "operationId": "saas.backupStatus",
            "summary": "Backup/retention posture for the caller's own tenant. `last_verified_restore: null` means no restore has ever been verified.",
            "tags": ["ops"],
            "security": [{"bearerAuth": []}],
            "responses": { "200": ok("Backup status.", "BackupStatus"), "401": unauthorized, "403": forbidden },
        }},
    })
}

/// Paths that are DESCRIBED but not served, and are therefore not
/// merged into the published contract.
///
/// Keeping them here, out of `paths()`, is the point: the description
/// is not lost, and the contract does not claim a deployment answers on
/// them. When a router serves one, move it into `paths()` — the
/// conformance test will then hold it to that promise.
pub fn unrouted_paths_pending_implementation() -> Value {
    json!({
        "/api/ops/preflight": { "note": "builder: crate::ops::deployment_preflight::evaluate — no router" },
        "/api/ops/runtime-config": { "note": "builder: crate::ops::runtime_config_report::build_report — no router" },
        "/api/ops/dependency-health": { "note": "builder: crate::ops::dependency_health::aggregate — no router" },
        "/api/ops/release-artifact": { "note": "builder: crate::ops::release_artifact::ReleaseArtifact — no router" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_no_secrets() {
        let s = format!("{}{}", schemas(), unrouted_schemas_pending_implementation())
            .to_ascii_lowercase();
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

    /// The published set and the unrouted set must stay disjoint: a path
    /// cannot be both "served" and "pending implementation".
    #[test]
    fn published_and_unrouted_paths_are_disjoint() {
        let published = paths();
        let pending = unrouted_paths_pending_implementation();
        for key in pending.as_object().expect("object").keys() {
            assert!(
                published.get(key).is_none(),
                "{key} is listed as unrouted but is also published"
            );
        }
    }
}
