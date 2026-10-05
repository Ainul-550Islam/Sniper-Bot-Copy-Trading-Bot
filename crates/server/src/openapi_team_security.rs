//! OpenAPI v3 component schemas and paths for Team, Security, Webhooks & Reports (SECOND.md §91).

use serde_json::{json, Value};

/// Generates OpenAPI v3 specification components for Team & Security.
pub fn team_security_schemas() -> Value {
    json!({
        "TeamInvite": {
            "type": "object",
            "required": ["email", "role"],
            "properties": {
                "email": { "type": "string", "format": "email" },
                "role": { "type": "string", "enum": ["owner", "admin", "operator", "analyst", "viewer"] }
            }
        },
        "SecurityPosture": {
            "type": "object",
            "required": ["organization_id", "mfa_enforced", "ip_allowlist_count", "active_signers", "kms_type"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "mfa_enforced": { "type": "boolean" },
                "ip_allowlist_count": { "type": "integer" },
                "active_signers": { "type": "integer" },
                "kms_type": { "type": "string" }
            }
        },
        "WebhookEndpoint": {
            "type": "object",
            "required": ["id", "organization_id", "url", "events", "is_active", "created_at"],
            "properties": {
                "id": { "type": "string" },
                "organization_id": { "type": "string", "format": "uuid" },
                "url": { "type": "string", "format": "uri" },
                "description": { "type": "string" },
                "events": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "secret": { "type": "string" },
                "is_active": { "type": "boolean" },
                "created_at": { "type": "string", "format": "date-time" }
            }
        },
        "ComplianceReport": {
            "type": "object",
            "required": ["id", "organization_id", "name", "type", "format", "size_bytes", "record_count", "created_at", "download_url"],
            "properties": {
                "id": { "type": "string" },
                "organization_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "type": { "type": "string" },
                "format": { "type": "string" },
                "size_bytes": { "type": "integer" },
                "record_count": { "type": "integer" },
                "created_at": { "type": "string", "format": "date-time" },
                "download_url": { "type": "string" }
            }
        }
    })
}

/// Generates OpenAPI v3 paths documentation for Team, Security, Webhooks & Reports endpoints.
pub fn team_security_paths() -> Value {
    json!({
        "/api/saas/team/invites": {
            "post": {
                "summary": "Invite member to tenant organization",
                "tags": ["Team & Access"],
                "responses": {
                    "201": { "description": "Invitation dispatched" },
                    "400": { "description": "Invalid email" },
                    "401": { "description": "Unauthorized" },
                    "403": { "description": "Forbidden" }
                }
            }
        },
        "/api/saas/security/rotate-tokens": {
            "post": {
                "summary": "Rotate all organization credentials and sessions",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "Tokens rotated successfully" }
                }
            }
        },
        "/api/saas/webhooks": {
            "get": {
                "summary": "List configured outbound webhooks",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "Webhooks list returned" }
                }
            },
            "post": {
                "summary": "Register webhook endpoint",
                "tags": ["Webhooks"],
                "responses": {
                    "201": { "description": "Webhook registered" }
                }
            }
        },
        "/api/saas/reports": {
            "get": {
                "summary": "List audit and compliance reports",
                "tags": ["Reports"],
                "responses": {
                    "200": { "description": "Reports list returned" }
                }
            }
        }
    })
}
