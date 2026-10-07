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
        "/api/saas/team/invites/accept": {
            "post": {
                "operationId": "saas.acceptInvite",
                "summary": "Accept a one-time tenant invitation",
                "tags": ["Team & Access"],
                "security": [],
                "responses": {
                    "200": { "description": "Invitation accepted and session issued" },
                    "400": { "description": "Invalid password or invitation token" },
                    "404": { "description": "Invitation not found or expired" },
                    "409": { "description": "Invitation already used or membership already exists" },
                    "503": { "description": "Identity storage unavailable" }
                }
            }
        },
        "/api/saas/team/invites/{id}": {
            "delete": {
                "summary": "Revoke a pending tenant invitation",
                "tags": ["Team & Access"],
                "responses": {
                    "200": { "description": "Invitation revoked" },
                    "404": { "description": "Invitation not found" }
                }
            }
        },
        "/api/saas/team/invites/{id}/resend": {
            "post": {
                "summary": "Rotate and resend a tenant invitation token",
                "tags": ["Team & Access"],
                "responses": {
                    "200": { "description": "Invitation token returned once" },
                    "404": { "description": "Invitation not found" }
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
        "/api/saas/security/status": {
            "get": {
                "summary": "Read tenant security posture",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "Security posture returned" }
                }
            }
        },
        "/api/saas/security/mfa-enforce": {
            "post": {
                "summary": "Enable or disable tenant MFA enforcement",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "MFA policy updated" },
                    "409": { "description": "A verified TOTP device is required" }
                }
            }
        },
        "/api/saas/security/totp/setup": {
            "post": {
                "summary": "Create an encrypted TOTP enrollment",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "One-time TOTP enrollment secret returned" }
                }
            }
        },
        "/api/saas/security/totp/verify": {
            "post": {
                "summary": "Verify a TOTP enrollment code",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "TOTP device verified" },
                    "401": { "description": "Invalid or replayed code" }
                }
            }
        },
        "/api/saas/security/ip-allowlist": {
            "post": {
                "summary": "Replace the tenant IP CIDR allowlist",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "IP allowlist updated" }
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
        },
        "/api/saas/reports/export": {
            "post": {
                "summary": "Generate and persist a tenant report export",
                "tags": ["Reports"],
                "responses": {
                    "201": { "description": "Report export created" }
                }
            }
        },
        "/api/saas/reports/{id}/download": {
            "get": {
                "summary": "Download a tenant report export",
                "tags": ["Reports"],
                "responses": {
                    "200": { "description": "Report bytes returned" },
                    "404": { "description": "Report not found" }
                }
            }
        },
        "/api/saas/webhooks/{id}": {
            "delete": {
                "summary": "Delete a tenant webhook endpoint",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "Webhook deleted" }
                }
            }
        },
        "/api/saas/webhooks/{id}/rotate": {
            "post": {
                "summary": "Rotate a tenant webhook secret",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "One-time webhook secret returned" }
                }
            }
        },
        "/api/saas/webhooks/{id}/test": {
            "post": {
                "summary": "Send a signed webhook test event",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "Webhook test delivered" },
                    "502": { "description": "Delivery failed" }
                }
            }
        }
    })
}
