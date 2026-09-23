//! Public SaaS API contract (TASK 7B file 13).
//!
//! One document, [`document`], describes every customer-facing SaaS route
//! with stable `operationId`s, request/response schemas, the auth
//! requirement and the shared error shape. It is served at
//! `GET /api/saas/openapi.json` without authentication: a contract is
//! public by definition and the document contains no secrets — credential
//! fields are request-only (`writeOnly`) and no response schema carries a
//! hash, token or secret.
//!
//! Internal-only endpoints (deployment key routes, `/api/kill`, journal,
//! metrics, the legacy `/api/events` stream, the PostgreSQL probe) are
//! deliberately absent: this document is the tenant-facing contract.

use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// One reusable non-success response.
fn error_ref() -> Value {
    json!({ "$ref": "#/components/schemas/Error" })
}

/// A JSON response declaration.
fn json_response(description: &str, schema: Value) -> Value {
    json!({ "description": description, "content": { "application/json": { "schema": schema } } })
}

/// One operation, with the stable identity consumers code against.
fn operation(
    id: &str,
    summary: &str,
    tags: &[&str],
    auth: bool,
    request: Value,
    responses: Value,
) -> Value {
    let mut op = json!({
        "operationId": id,
        "summary": summary,
        "tags": tags,
        "responses": responses,
    });
    if auth {
        op["security"] = json!([{ "bearerAuth": [] }]);
    }
    if !request.is_null() {
        op["requestBody"] = json!({
            "required": true,
            "content": { "application/json": { "schema": request } },
        });
    }
    op
}

/// Standard `4xx` responses every authenticated operation may return.
fn guarded_responses(ok: &str, schema: Value) -> Value {
    json!({
        "200": json_response(ok, schema),
        "401": error_ref(),
        "403": error_ref(),
        "429": error_ref(),
    })
}

/// The OpenAPI document for the SaaS control plane.
pub fn document() -> Value {
    let tenant_path = |name: &str| json!([{ "name": name, "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }]);

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Sniper Suite SaaS Control Plane",
            "version": "0.1.0",
            "description": "Multi-tenant SaaS contract over the TASK 7A control plane. \
                Trading truth (orders, fills, positions, risk, ledger, HA) remains owned by \
                the TASK 1–6 engines and is exposed read-only to tenants. Billing/usage \
                reads are served through the deterministic exports \
                (`/api/saas/exports?kind=subscription|usage|...`); there are no separate \
                plan-catalogue or usage endpoints.",
        },
        "servers": [ { "url": "/", "description": "the control-plane deployment itself" } ],
        "security": [ { "bearerAuth": [] } ],
        "tags": [
            { "name": "auth" }, { "name": "users" }, { "name": "organizations" },
            { "name": "api-keys" }, { "name": "billing" },
            { "name": "wallet-access" }, { "name": "exports" }, { "name": "events" },
            { "name": "contract" },
        ],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "A SaaS session token (login) or a tenant API key. \
                        The tenant context is the credential's own organization unless the \
                        caller is a platform administrator.",
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error", "reason"],
                    "properties": {
                        "error": { "type": "string", "description": "Stable refusal kind (deny_* / invalid_credentials / …)." },
                        "reason": { "type": "string", "description": "Single-line, secret-free human explanation." },
                    },
                },
                "UserProfile": {
                    "type": "object",
                    "required": ["id", "email", "email_verified", "display_name", "status", "platform_admin", "created_at"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "email": { "type": "string", "format": "email" },
                        "email_verified": { "type": "boolean" },
                        "display_name": { "type": "string" },
                        "status": { "type": "string", "enum": ["active", "suspended", "deactivated"] },
                        "platform_admin": { "type": "boolean" },
                        "created_at": { "type": "string", "format": "date-time" },
                        "last_login_at": { "type": ["string", "null"], "format": "date-time" },
                    },
                },
                "Organization": {
                    "type": "object",
                    "required": ["id", "slug", "name", "status", "created_at"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "slug": { "type": "string" },
                        "name": { "type": "string" },
                        "status": { "type": "string", "enum": ["active", "trialing", "past_due", "suspended", "closed"] },
                        "created_at": { "type": "string", "format": "date-time" },
                    },
                },
                "Membership": {
                    "type": "object",
                    "required": ["organization_id", "user_id", "role", "status"],
                    "properties": {
                        "organization_id": { "type": "string", "format": "uuid" },
                        "user_id": { "type": "string", "format": "uuid" },
                        "role": { "type": "string", "enum": ["platform_admin", "org_owner", "org_admin", "trader", "security_admin", "billing_admin", "auditor", "viewer"] },
                        "status": { "type": "string", "enum": ["active", "suspended", "removed"] },
                        "created_at": { "type": "string", "format": "date-time" },
                    },
                },
                "ApiKeyMetadata": {
                    "type": "object",
                    "description": "Metadata view. The secret material exists only in the create response, once.",
                    "required": ["id", "key_prefix", "label", "role", "created_at", "usable"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "key_prefix": { "type": "string" },
                        "label": { "type": "string" },
                        "role": { "type": "string" },
                        "scopes": { "type": "array", "items": { "type": "string" } },
                        "created_at": { "type": "string", "format": "date-time" },
                        "expires_at": { "type": ["string", "null"], "format": "date-time" },
                        "revoked_at": { "type": ["string", "null"], "format": "date-time" },
                        "usable": { "type": "boolean" },
                    },
                },
                "WalletBinding": {
                    "type": "object",
                    "description": "A tenant's authorized wallet binding. Public address only — no signing material exists in the SaaS layer.",
                    "required": ["id", "organization_id", "label", "public_address", "modules", "created_at"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "organization_id": { "type": "string", "format": "uuid" },
                        "label": { "type": "string" },
                        "public_address": { "type": "string" },
                        "modules": { "type": "array", "items": { "type": "string", "enum": ["module.sniper", "module.copy", "module.polymarket"] } },
                        "created_at": { "type": "string", "format": "date-time" },
                        "revoked_at": { "type": ["string", "null"], "format": "date-time" },
                    },
                },
                "ExportEnvelope": {
                    "type": "object",
                    "required": ["organization_id", "kind", "generated_at", "data"],
                    "properties": {
                        "organization_id": { "type": "string", "format": "uuid" },
                        "kind": { "type": "string", "enum": ["profile", "members", "api_keys", "usage", "subscription", "wallets", "audit"] },
                        "generated_at": { "type": "string", "format": "date-time" },
                        "data": { "description": "Deterministic, sorted section payload.", "type": "object", "additionalProperties": true },
                    },
                },
                "WebhookAck": {
                    "type": "object",
                    "required": ["status"],
                    "properties": {
                        "status": { "type": "string", "enum": ["applied", "ignored", "duplicate", "rejected"] },
                        "detail": { "type": "string" },
                        "reason": { "type": "string" },
                    },
                },
            },
        },
        "paths": {
            "/api/saas/users": {
                "post": operation(
                    "saas.registerUser",
                    "Register a user (email + PBKDF2-hashed password).",
                    &["users"], false,
                    json!({ "type": "object", "required": ["email", "password"], "properties": {
                        "email": { "type": "string", "format": "email" },
                        "password": { "type": "string", "writeOnly": true, "minLength": 12 },
                        "display_name": { "type": "string" },
                    } }),
                    json!({ "201": json_response("The created user profile.", json!({ "$ref": "#/components/schemas/UserProfile" })), "409": error_ref() }),
                ),
            },
            "/api/saas/sessions": {
                "post": operation(
                    "saas.login",
                    "Create a session. The token is returned exactly once and never stored server-side in plaintext.",
                    &["auth"], false,
                    json!({ "type": "object", "required": ["email", "password"], "properties": {
                        "email": { "type": "string", "format": "email" },
                        "password": { "type": "string", "writeOnly": true },
                    } }),
                    json!({
                        "200": json_response("Session created; `token` is the one-time secret.", json!({
                            "type": "object", "properties": {
                                "user": { "$ref": "#/components/schemas/UserProfile" },
                                "token": { "type": "string", "writeOnly": true },
                                "session": { "type": "object", "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "prefix": { "type": "string" },
                                    "expires_at": { "type": "string", "format": "date-time" },
                                } },
                            },
                        })),
                        "401": error_ref(),
                    }),
                ),
            },
            "/api/saas/users/me": {
                "get": operation(
                    "saas.currentUser",
                    "The authenticated user plus the organizations they belong to.",
                    &["users"], true, json!(null),
                    guarded_responses("The user profile and tenant memberships.", json!({
                        "type": "object",
                        "properties": {
                            "user": { "$ref": "#/components/schemas/UserProfile" },
                            "organizations": { "type": "array", "items": { "type": "object", "properties": {
                                "organization_id": { "type": "string", "format": "uuid" },
                                "slug": { "type": "string" },
                                "name": { "type": "string" },
                                "status": { "type": "string" },
                                "role": { "type": "string" },
                            } } },
                        },
                    })),
                ),
                "patch": operation(
                    "saas.updateProfile",
                    "Update safe profile fields of the current user.",
                    &["users"], true,
                    json!({ "type": "object", "properties": { "display_name": { "type": "string" } } }),
                    guarded_responses("The updated profile.", json!({ "$ref": "#/components/schemas/UserProfile" })),
                ),
            },
            "/api/saas/users/me/logout": {
                "post": operation(
                    "saas.logout", "Revoke the presented session.", &["auth"], true, json!(null),
                    guarded_responses("Session revoked.", json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } })),
                ),
            },
            "/api/saas/organizations": {
                "post": operation(
                    "saas.createOrganization",
                    "Create an organization through the provisioning state machine; the creator becomes its owner.",
                    &["organizations"], true,
                    json!({ "type": "object", "required": ["slug", "name"], "properties": {
                        "slug": { "type": "string", "pattern": "^[a-z0-9][a-z0-9-]{1,62}$" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 120 },
                    } }),
                    json!({ "201": json_response("The organization.", json!({ "$ref": "#/components/schemas/Organization" })), "409": error_ref() }),
                ),
            },
            "/api/saas/organizations/{id}": {
                "parameters": tenant_path("id"),
                "get": operation(
                    "saas.getOrganization", "One organization the caller may see.", &["organizations"], true, json!(null),
                    guarded_responses("The organization.", json!({ "$ref": "#/components/schemas/Organization" })),
                ),
                "patch": operation(
                    "saas.updateOrganization", "Update the organization record (tenant.update).", &["organizations"], true,
                    json!({ "type": "object", "properties": { "name": { "type": "string" } } }),
                    guarded_responses("The updated organization.", json!({ "$ref": "#/components/schemas/Organization" })),
                ),
            },
            "/api/saas/organizations/{id}/members": {
                "parameters": tenant_path("id"),
                "get": operation(
                    "saas.listMembers", "The organization's members (users.read).", &["organizations"], true, json!(null),
                    guarded_responses("Member list.", json!({ "type": "array", "items": { "$ref": "#/components/schemas/Membership" } })),
                ),
            },
            "/api/saas/organizations/{id}/suspension": {
                "parameters": tenant_path("id"),
                "post": operation(
                    "saas.suspendOrganization", "Suspend or restore the organization (platform staff).", &["organizations"], true,
                    json!({ "type": "object", "required": ["suspended"], "properties": {
                        "suspended": { "type": "boolean" }, "reason": { "type": "string" },
                    } }),
                    guarded_responses("The organization after the change.", json!({ "$ref": "#/components/schemas/Organization" })),
                ),
            },
            "/api/saas/api-keys": {
                "post": operation(
                    "saas.createApiKey",
                    "Create a tenant API key. The response carries the plaintext secret EXACTLY ONCE; only its hash is stored.",
                    &["api-keys"], true,
                    json!({ "type": "object", "required": ["label"], "properties": {
                        "label": { "type": "string", "minLength": 1, "maxLength": 120 },
                        "role": { "type": "string", "enum": ["org_admin", "trader", "security_admin", "billing_admin", "auditor", "viewer"] },
                        "scopes": { "type": "array", "items": { "type": "string" } },
                        "expires_in_days": { "type": "integer", "minimum": 1, "maximum": 3650 },
                    } }),
                    json!({
                        "201": json_response("The key metadata plus the one-time `secret`.", json!({
                            "type": "object",
                            "required": ["key", "secret"],
                            "properties": {
                                "key": { "$ref": "#/components/schemas/ApiKeyMetadata" },
                                "secret": { "type": "string", "writeOnly": true },
                            },
                        })),
                        "403": error_ref(),
                    }),
                ),
                "get": operation(
                    "saas.listApiKeys", "The tenant's API keys (metadata only — never secrets).", &["api-keys"], true, json!(null),
                    guarded_responses("Key metadata list.", json!({ "type": "array", "items": { "$ref": "#/components/schemas/ApiKeyMetadata" } })),
                ),
            },
            "/api/saas/api-keys/{prefix}": {
                "parameters": tenant_path("prefix"),
                "delete": operation(
                    "saas.revokeApiKey", "Revoke a tenant API key (api_key.revoke).", &["api-keys"], true, json!(null),
                    guarded_responses("Revocation acknowledged.", json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } })),
                ),
            },
            "/api/saas/wallet-access": {
                "post": operation(
                    "saas.bindWallet",
                    "Register a tenant wallet binding (wallet.manage). Public address only — the SaaS layer never receives signing material.",
                    &["wallet-access"], true,
                    json!({ "type": "object", "required": ["label", "public_address"], "properties": {
                        "label": { "type": "string", "minLength": 1, "maxLength": 120 },
                        "public_address": { "type": "string", "minLength": 8, "maxLength": 100, "pattern": "^[A-Za-z0-9:_-]+$" },
                        "modules": { "type": "array", "items": { "type": "string", "enum": ["module.sniper", "module.copy", "module.polymarket"] } },
                    } }),
                    json!({ "201": json_response("The binding.", json!({ "$ref": "#/components/schemas/WalletBinding" })), "403": error_ref() }),
                ),
                "get": operation(
                    "saas.listWallets", "The tenant's wallet bindings (wallet.read).", &["wallet-access"], true, json!(null),
                    guarded_responses("Bindings for the caller's own tenant only.", json!({ "type": "array", "items": { "$ref": "#/components/schemas/WalletBinding" } })),
                ),
            },
            "/api/saas/wallet-access/{id}": {
                "parameters": tenant_path("id"),
                "delete": operation(
                    "saas.revokeWallet", "Revoke a wallet binding of the caller's own tenant (wallet.manage).", &["wallet-access"], true, json!(null),
                    guarded_responses("Revocation acknowledged.", json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } })),
                ),
            },
            "/api/saas/wallet-access/{id}/authorize": {
                "parameters": tenant_path("id"),
                "post": operation(
                    "saas.authorizeWalletModule",
                    "Ask the boundary whether this tenant may run `module` against this wallet now (ownership ∧ binding ∧ entitlement).",
                    &["wallet-access"], true,
                    json!({ "type": "object", "required": ["module"], "properties": {
                        "module": { "type": "string", "enum": ["module.sniper", "module.copy", "module.polymarket"] },
                    } }),
                    guarded_responses("The boundary decision.", json!({
                        "type": "object", "required": ["allowed"],
                        "properties": { "allowed": { "type": "boolean" }, "reason": { "type": "string" }, "binding": { "$ref": "#/components/schemas/WalletBinding" } },
                    })),
                ),
            },
            "/api/saas/exports": {
                "get": operation(
                    "saas.exportData",
                    "Export one deterministic, tenant-scoped section (export.create; audit section additionally needs audit.read).",
                    &["exports"], true, json!(null),
                    json!({
                        "200": json_response("The export envelope.", json!({ "$ref": "#/components/schemas/ExportEnvelope" })),
                        "401": error_ref(), "403": error_ref(),
                    }),
                ),
            },
            "/api/saas/events": {
                "get": operation(
                    "saas.streamEvents",
                    "Authenticated tenant-scoped WebSocket stream. No credential may travel in the URL: authenticate with the \
                     `Authorization: Bearer` header, or send {\"type\":\"auth\",\"token\":…} as the first frame after connecting.",
                    &["events"], true, json!(null),
                    json!({
                        "101": { "description": "Upgraded; the first frame must authenticate." },
                        "401": error_ref(),
                    }),
                ),
            },
            "/api/saas/billing/webhooks/{provider}": {
                "parameters": [ { "name": "provider", "in": "path", "required": true, "schema": { "type": "string", "enum": ["stripe", "paddle"] } } ],
                "post": operation(
                    "saas.billingWebhook",
                    "Provider webhook entry. Signature-verified and idempotent by provider event id; not user-authenticated.",
                    &["billing"], false,
                    json!({ "type": "object", "required": ["id", "type"], "properties": {
                        "id": { "type": "string" }, "type": { "type": "string" }, "data": { "type": "object" },
                    } }),
                    json!({
                        "200": json_response("applied | ignored | duplicate", json!({ "$ref": "#/components/schemas/WebhookAck" })),
                        "401": error_ref(), "422": json_response("rejected", json!({ "$ref": "#/components/schemas/WebhookAck" })),
                        "501": error_ref(),
                    }),
                ),
            },
        },
    })
}

/// `GET /api/saas/openapi.json` — the public contract.
pub async fn serve() -> Response {
    Json(document()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_stable_and_unique() {
        let doc = document();
        let mut ids = Vec::new();
        for (path, item) in doc["paths"].as_object().expect("paths") {
            for (method, op) in item.as_object().expect("path item") {
                if method == "parameters" {
                    continue;
                }
                let id = op["operationId"].as_str().unwrap_or_else(|| {
                    panic!("every operation needs an operationId ({path} {method})")
                });
                ids.push(id.to_string());
            }
        }
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "operationIds must be unique");
        assert!(
            total >= 20,
            "the contract covers the whole SaaS surface ({total})"
        );
    }

    #[test]
    fn no_response_schema_carries_secret_material() {
        let text = serde_json::to_string(&document()).unwrap();
        // Response schemas may describe the ONE-TIME create secret and the
        // login token as writeOnly; they must never name the STORED forms.
        for banned in ["secret_hash", "password_hash", "token_hash"] {
            assert!(
                !text.contains(banned),
                "the contract must not expose {banned}"
            );
        }
        assert!(
            text.contains("\"writeOnly\""),
            "one-time secrets are writeOnly"
        );
    }

    #[test]
    fn internal_endpoints_are_not_in_the_public_contract() {
        let text = serde_json::to_string(&document()).unwrap();
        for internal in [
            "/api/kill",
            "/api/journal",
            "/api/metrics",
            "/api/keys",
            "/api/db",
            "/api/resume",
        ] {
            assert!(
                !text.contains(internal),
                "internal endpoint {internal} leaked into the contract"
            );
        }
    }

    #[test]
    fn every_authenticated_path_declares_security() {
        let doc = document();
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                if method == "parameters" {
                    continue;
                }
                let public = matches!(
                    op["operationId"].as_str().unwrap(),
                    "saas.registerUser" | "saas.login" | "saas.billingWebhook"
                );
                let has_security = op.get("security").is_some() || doc.get("security").is_some();
                assert!(has_security, "{path} {method} must declare its auth");
                if public {
                    assert!(
                        op.get("security").is_none(),
                        "{path} {method} is public and must not override with credentials"
                    );
                }
            }
        }
    }
}
