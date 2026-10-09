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
        "TotpEnrollment": {
            "type": "object",
            "required": ["device_id", "secret", "otpauth_url", "verified"],
            "properties": {
                "device_id": { "type": "string", "format": "uuid" },
                "secret": { "type": "string", "writeOnly": true, "description": "Shown once during enrollment; never returned by status or device-list endpoints." },
                "otpauth_url": { "type": "string", "writeOnly": true },
                "verified": { "type": "boolean" },
                "backup_codes": { "type": ["array", "null"], "items": { "type": "string", "writeOnly": true } },
                "backup_codes_status": { "type": "string" }
            }
        },
        "TotpVerification": {
            "type": "object",
            "required": ["success", "device_id", "verified", "session_promoted"],
            "properties": {
                "success": { "type": "boolean" },
                "device_id": { "type": "string", "format": "uuid" },
                "verified": { "type": "boolean" },
                "session_promoted": { "type": "boolean" }
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
                "operationId": "saas.createInvite",
                "summary": "Invite member to tenant organization",
                "tags": ["Team & Access"],
                "responses": {
                    "201": { "description": "Invitation dispatched", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TeamInvite" } } } },
                    "400": { "description": "Invalid email" },
                    "401": { "description": "Unauthorized" },
                    "403": { "description": "Forbidden" }
                }
            }
        },
        "/api/saas/team/invites/accept": {
            "post": {
                "operationId": "saas.acceptInvite",
                "summary": "Accept a one-time tenant invitation; MFA-protected tenants receive an enrollment-only session",
                "tags": ["Team & Access"],
                "security": [],
                "requestBody": {
                    "required": true,
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["token"],
                        "properties": {
                            "token": { "type": "string", "writeOnly": true },
                            "password": { "type": "string", "writeOnly": true, "minLength": 12 },
                            "display_name": { "type": "string", "maxLength": 160 }
                        }
                    } } }
                },
                "responses": {
                    "200": {
                        "description": "Invitation accepted; returned token is restricted to MFA enrollment when required",
                        "content": { "application/json": { "schema": {
                            "type": "object",
                            "required": ["user", "token", "session", "organization_id", "organization_slug", "mfa_enrollment_required", "membership"],
                            "properties": {
                                "user": { "$ref": "#/components/schemas/UserProfile" },
                                "token": { "type": "string", "writeOnly": true },
                                "session": { "type": "object", "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "prefix": { "type": "string" },
                                    "expires_at": { "type": "string", "format": "date-time" },
                                    "organization_id": { "type": "string", "format": "uuid" }
                                } },
                                "organization_id": { "type": "string", "format": "uuid" },
                                "organization_slug": { "type": "string" },
                                "mfa_enrollment_required": { "type": "boolean" },
                                "membership": { "type": "object", "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "role": { "type": "string" },
                                    "status": { "type": "string" }
                                } }
                            }
                        } } }
                    },
                    "400": { "description": "Invalid password or invitation token" },
                    "404": { "description": "Invitation not found or expired" },
                    "409": { "description": "Invitation already used or membership already exists" },
                    "429": { "description": "Invitation acceptance is rate limited" },
                    "503": { "description": "Identity storage or mandatory MFA enrollment unavailable" }
                }
            }
        },
        "/api/saas/team/invites/{id}": {
            "delete": {
                "operationId": "saas.revokeInvite",
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
                "operationId": "saas.resendInvite",
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
                "operationId": "saas.rotateTokens",
                "summary": "Rotate all organization credentials and sessions",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "Tokens rotated successfully" }
                }
            }
        },
        "/api/saas/security/status": {
            "get": {
                "operationId": "saas.getSecurityStatus",
                "summary": "Read tenant security posture",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "Security posture returned", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/SecurityPosture" } } } }
                }
            }
        },
        "/api/saas/security/mfa-enforce": {
            "post": {
                "operationId": "saas.setMfaEnforcement",
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
                "operationId": "saas.setupTotp",
                "summary": "Create an encrypted TOTP enrollment",
                "tags": ["Security"],
                "responses": {
                    "200": {
                        "description": "One-time TOTP enrollment secret returned; secret is write-only and must be stored by the authenticator app",
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TotpEnrollment" } } }
                    },
                    "429": { "description": "TOTP setup is rate limited" },
                    "503": { "description": "MFA encryption or storage unavailable" }
                }
            }
        },
        "/api/saas/security/totp/verify": {
            "post": {
                "operationId": "saas.verifyTotp",
                "summary": "Verify a TOTP enrollment code and promote a restricted session",
                "tags": ["Security"],
                "requestBody": {
                    "required": true,
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["device_id", "code"],
                        "properties": {
                            "device_id": { "type": "string", "format": "uuid" },
                            "code": { "type": "string", "pattern": "^[0-9]{6}$" }
                        }
                    } } }
                },
                "responses": {
                    "200": {
                        "description": "TOTP device verified; session_promoted is true for onboarding sessions",
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TotpVerification" } } }
                    },
                    "401": { "description": "Invalid code" },
                    "409": { "description": "TOTP counter already used" },
                    "429": { "description": "TOTP verification is rate limited" },
                    "503": { "description": "TOTP state or session activation unavailable" }
                }
            }
        },
        "/api/saas/security/ip-allowlist": {
            "post": {
                "operationId": "saas.setIpAllowlist",
                "summary": "Replace the tenant IP CIDR allowlist",
                "tags": ["Security"],
                "responses": {
                    "200": { "description": "IP allowlist updated" }
                }
            }
        },
        "/api/saas/webhooks": {
            "get": {
                "operationId": "saas.listWebhooks",
                "summary": "List configured outbound webhooks",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "Webhooks list returned", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/WebhookEndpoint" } } } } }
                }
            },
            "post": {
                "operationId": "saas.createWebhook",
                "summary": "Register webhook endpoint",
                "tags": ["Webhooks"],
                "responses": {
                    "201": { "description": "Webhook registered", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WebhookEndpoint" } } } }
                }
            }
        },
        "/api/saas/reports": {
            "get": {
                "operationId": "saas.listReports",
                "summary": "List audit and compliance reports",
                "tags": ["Reports"],
                "responses": {
                    "200": { "description": "Reports list returned" }
                }
            }
        },
        "/api/saas/reports/export": {
            "post": {
                "operationId": "saas.exportReport",
                "summary": "Generate and persist a tenant report export",
                "tags": ["Reports"],
                "responses": {
                    "201": { "description": "Report export created", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ComplianceReport" } } } }
                }
            }
        },
        "/api/saas/reports/{id}/download": {
            "get": {
                "operationId": "saas.downloadReport",
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
                "operationId": "saas.deleteWebhook",
                "summary": "Delete a tenant webhook endpoint",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "Webhook deleted" }
                }
            }
        },
        "/api/saas/webhooks/{id}/rotate": {
            "post": {
                "operationId": "saas.rotateWebhookSecret",
                "summary": "Rotate a tenant webhook secret",
                "tags": ["Webhooks"],
                "responses": {
                    "200": { "description": "One-time webhook secret returned" }
                }
            }
        },
        "/api/saas/webhooks/{id}/test": {
            "post": {
                "operationId": "saas.testWebhook",
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
