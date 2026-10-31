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
//!
//! # Fragments (P1 — API contract completeness)
//!
//! The billing, commercial, custody and ops surfaces describe themselves
//! in `crate::api::openapi_{billing,commercial,custody,ops}`. Those four
//! modules existed but **were never merged into this document** — they
//! were compiled, never called, so roughly 760 lines of published
//! contract were missing from `/api/saas/openapi.json` while the SDK and
//! the docs implied they were there. [`merge_api_fragments`] now folds
//! them in, and `openapi_artifact.rs` asserts the exported artifact stays
//! in lockstep.
//!
//! # Versioning
//!
//! [`API_VERSION`] is the contract version and is NOT the crate version:
//! a patch release of the binary must not look like an API change. The
//! compatibility policy is in `docs/API-VERSIONING.md` and is enforced by
//! the committed artifact diff — a breaking change cannot land unnoticed
//! because the artifact changes with it.

use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// The version of the PUBLISHED CONTRACT.
///
/// Deliberately independent of `CARGO_PKG_VERSION`: the binary ships
/// patches that change no route, and a consumer that pins the contract
/// must not be told the API moved because a dependency was bumped. Bump
/// this according to `docs/API-VERSIONING.md`:
///   * PATCH — documentation/description only;
///   * MINOR — new path, new optional field, new enum value (additive);
///   * MAJOR — anything a conforming client could break on.
///
/// 2.0.0 — `BackupStatus` was rebuilt around the backup ledger. The
/// fields `retention_configured` and `protection` are GONE, not
/// deprecated: a deployment that is not backing anything up had no
/// honest value to put in them. A conforming 1.x client breaks on this,
/// which is exactly what a MAJOR bump is for. See
/// `docs/API-VERSIONING.md` and the CHANGELOG entry for the migration.
pub const API_VERSION: &str = "2.4.0";

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

/// A genuinely public operation: unlike an omitted operation-level security
/// declaration, an empty array overrides the document-wide bearer default.
fn public_operation(
    id: &str,
    summary: &str,
    tags: &[&str],
    request: Value,
    responses: Value,
) -> Value {
    let mut op = operation(id, summary, tags, false, request, responses);
    op["security"] = json!([]);
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
///
/// This is `base_document()` with the per-surface fragments merged in;
/// the split exists so the base and the fragments can be inspected
/// independently (e.g. to report every conflict at once instead of the
/// first one).
pub fn document() -> Value {
    let mut doc = base_document();
    merge_api_fragments(&mut doc);
    doc
}

/// The hand-written core of the contract, before fragments are merged.
pub fn base_document() -> Value {
    let tenant_path = |name: &str| json!([{ "name": name, "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }]);

    let doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Sniper Suite SaaS Control Plane",
            "version": API_VERSION,
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
            { "name": "checkout" }, { "name": "invoices" },
            { "name": "custody" }, { "name": "lifecycle" },
            { "name": "wallet-access" }, { "name": "exports" }, { "name": "events" },
            { "name": "contract" }, { "name": "reports" },
            { "name": "tenant" }, { "name": "operator" },
        ],
        "components": {
            // Reusable responses. The surface fragments in `crate::api`
            // reference these by name; before they existed those `$ref`s
            // dangled, which every OpenAPI validator and SDK generator
            // treats as a broken document.
            "responses": {
                "Unauthorized": {
                    "description": "Missing, malformed or expired credential.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } },
                },
                "Forbidden": {
                    "description": "Authenticated, but the credential may not act on this resource. \
                        Note: cross-TENANT access returns 404, not 403, so the API is not an existence oracle.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } },
                },
            },
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
                "UserSessionView": {
                    "type": "object",
                    "description": "Non-secret metadata for one of the authenticated user's sessions.",
                    "required": ["id", "prefix", "user_agent", "ip", "created_at", "last_seen_at", "expires_at", "revoked_at", "state", "current"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "prefix": { "type": "string", "description": "Non-secret token prefix for recognizing a session." },
                        "user_agent": { "type": "string" },
                        "ip": { "type": "string" },
                        "created_at": { "type": "string", "format": "date-time" },
                        "last_seen_at": { "type": "string", "format": "date-time" },
                        "expires_at": { "type": "string", "format": "date-time" },
                        "revoked_at": { "type": ["string", "null"], "format": "date-time" },
                        "state": { "type": "string", "enum": ["active", "expired", "revoked"] },
                        "current": { "type": "boolean", "description": "Whether this is the session authenticating the request." }
                    }
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
                        "organization": { "type": "string", "description": "Organization slug used to scope login and apply its MFA policy." },
                        "mfa_code": { "type": "string", "pattern": "^[0-9]{6}$", "description": "Current TOTP code when the selected organization enforces MFA." },
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
                                    "organization_id": { "type": ["string", "null"], "format": "uuid" },
                                } },
                                "mfa_enrollment_required": { "type": "boolean", "description": "True only for a restricted session that may complete TOTP enrollment." },
                                "organization_id": { "type": "string", "format": "uuid" },
                                "organization_slug": { "type": "string" },
                            },
                        })),
                        "401": error_ref(),
                        "409": error_ref(),
                        "429": error_ref(),
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
            "/api/saas/users/me/sessions": {
                "get": operation(
                    "saas.listSessions",
                    "List non-secret session metadata for the authenticated user's own sessions.",
                    &["auth"], true, json!(null),
                    guarded_responses("Session inventory for the authenticated user.", json!({
                        "type": "object",
                        "required": ["items", "count"],
                        "properties": {
                            "items": { "type": "array", "items": { "$ref": "#/components/schemas/UserSessionView" } },
                            "count": { "type": "integer" }
                        }
                    })),
                ),
            },
            "/api/saas/users/me/sessions/{id}": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "delete": operation(
                    "saas.revokeUserSession",
                    "Revoke one session owned by the authenticated user; another user's session is indistinguishable from a missing session.",
                    &["auth"], true, json!(null),
                    json!({
                        "200": json_response("Revocation result.", json!({
                            "type": "object",
                            "required": ["ok", "session_id", "revoked"],
                            "properties": {
                                "ok": { "type": "boolean" },
                                "session_id": { "type": "string", "format": "uuid" },
                                "revoked": { "type": "boolean", "description": "False if the session was already revoked." }
                            }
                        })),
                        "401": error_ref(), "403": error_ref(), "404": error_ref(), "429": error_ref(), "503": error_ref()
                    }),
                ),
            },
            "/api/saas/password-reset/request": {
                "post": operation(
                    "saas.requestPasswordReset",
                    "Request a password-reset email. The response is identical whether or not the address exists (no account enumeration); 202 means the request was accepted for processing.",
                    &["auth"], false,
                    json!({ "type": "object", "required": ["email"], "properties": {
                        "email": { "type": "string", "format": "email" },
                    } }),
                    json!({
                        "202": json_response("Accepted; identical body for known and unknown addresses.", json!({
                            "type": "object", "properties": {
                                "status": { "type": "string", "enum": ["accepted"] },
                                "message": { "type": "string" },
                                "expires_minutes": { "type": "integer" },
                            },
                        })),
                        "429": error_ref(),
                        "503": error_ref(),
                    }),
                ),
            },
            "/api/saas/password-reset/confirm": {
                "post": operation(
                    "saas.confirmPasswordReset",
                    "Consume a single-use reset token, set the new password and revoke every live session for the user.",
                    &["auth"], false,
                    json!({ "type": "object", "required": ["token", "new_password"], "properties": {
                        "token": { "type": "string", "writeOnly": true, "description": "The one-time token from the reset link." },
                        "new_password": { "type": "string", "writeOnly": true, "minLength": 12 },
                    } }),
                    json!({
                        "200": json_response("Password changed; all sessions revoked.", json!({
                            "type": "object", "properties": {
                                "status": { "type": "string", "enum": ["reset_complete"] },
                                "message": { "type": "string" },
                            },
                        })),
                        "422": error_ref(),
                        "429": error_ref(),
                        "503": error_ref(),
                    }),
                ),
            },
            "/api/saas/email-verification/request": {
                "post": operation(
                    "saas.requestEmailVerification",
                    "Request an email-verification link. The response is identical whether or not the address belongs to an unverified account (no account enumeration).",
                    &["auth"], false,
                    json!({ "type": "object", "required": ["email"], "properties": {
                        "email": { "type": "string", "format": "email" },
                    } }),
                    json!({
                        "202": json_response("Accepted; identical body in every case.", json!({
                            "type": "object", "properties": {
                                "status": { "type": "string", "enum": ["accepted"] },
                                "message": { "type": "string" },
                                "expires_minutes": { "type": "integer" },
                            },
                        })),
                        "429": error_ref(),
                        "503": error_ref(),
                    }),
                ),
            },
            "/api/saas/email-verification/confirm": {
                "post": operation(
                    "saas.confirmEmailVerification",
                    "Consume a single-use verification token and mark the account's email address as verified.",
                    &["auth"], false,
                    json!({ "type": "object", "required": ["token"], "properties": {
                        "token": { "type": "string", "writeOnly": true, "description": "The one-time token from the verification link." },
                    } }),
                    json!({
                        "200": json_response("Email address verified.", json!({
                            "type": "object", "properties": {
                                "status": { "type": "string", "enum": ["verified"] },
                                "message": { "type": "string" },
                            },
                        })),
                        "422": error_ref(),
                        "429": error_ref(),
                        "503": error_ref(),
                    }),
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
                    "saas.setOrganizationSuspension", "Suspend or restore the organization (platform staff).", &["organizations"], true,
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
            "/api/saas/billing/payment-webhooks/{provider}": {
                "parameters": [ { "name": "provider", "in": "path", "required": true, "schema": { "type": "string", "enum": ["stripe", "paddle"] } } ],
                "post": operation(
                    "saas.billingPaymentWebhook",
                    "Provider-neutral payment/invoice event entry. Signature-verified and idempotent; not user-authenticated.",
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
            "/api/saas/checkout": {
                "post": operation(
                    "saas.createCheckout",
                    "Create a provider-neutral checkout session for the caller's own tenant. Price authority is server-side plan definitions; client-supplied amounts are never trusted.",
                    &["checkout"], true,
                    // Typed by the billing fragment rather than copied inline:
                    // an inline duplicate is a second definition that drifts.
                    json!({ "$ref": "#/components/schemas/CheckoutRequest" }),
                    json!({
                        "201": json_response("Checkout created.", json!({ "$ref": "#/components/schemas/CheckoutResponse" })),
                        "400": error_ref(), "401": error_ref(), "403": error_ref(), "409": error_ref(),
                    }),
                ),
            },
            "/api/saas/invoices": {
                "get": operation(
                    "saas.listInvoices",
                    "List invoices for the caller's own tenant (tenant-scoped query).",
                    &["invoices"], true, json!(null),
                    guarded_responses("Invoice list.", json!({ "type": "object", "properties": {
                        "organization_id": { "type": "string", "format": "uuid" },
                        "invoices": { "type": "array", "items": { "$ref": "#/components/schemas/InvoiceView" } },
                        "count": { "type": "integer" },
                    } })),
                ),
            },
            "/api/saas/invoices/{id}": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "get": operation(
                    "saas.getInvoice",
                    "Get one invoice. Uses organization_id predicate so cross-tenant access returns 404, not 403, to avoid existence oracle.",
                    &["invoices"], true, json!(null),
                    json!({
                        "200": json_response("The invoice.", json!({ "$ref": "#/components/schemas/InvoiceView" })),
                        "401": error_ref(), "403": error_ref(), "404": error_ref(),
                    }),
                ),
            },
            // NOTE: `/api/saas/custody/profiles` is declared in
            // `crate::api::openapi_custody`, which types the responses
            // (`CustodyProfile`) and documents 409/422. It is merged in by
            // `merge_api_fragments`; declaring it here too would be two
            // sources of truth for one path.
            "/api/saas/custody/profiles/{id}/activate": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "post": operation("saas.activateCustodyProfile", "Activate a custody profile (pending -> active). Closed tenants cannot activate.", &["custody"], true, json!(null), guarded_responses("Activated.", json!({ "type": "object" }))),
            },
            "/api/saas/custody/signers": {
                "post": operation(
                    "saas.createSigner",
                    "Create a logical signer under a custody profile. Public address only — never private key material.",
                    &["custody"], true,
                    json!({ "type": "object", "required": ["custody_profile_id", "logical_identity", "public_address"], "properties": {
                        "custody_profile_id": { "type": "string", "format": "uuid" },
                        "logical_identity": { "type": "string" },
                        "public_address": { "type": "string" },
                        "capabilities": { "type": "array", "items": { "type": "string" } },
                    } }),
                    json!({ "201": json_response("Signer created.", json!({ "type": "object" })), "400": error_ref(), "403": error_ref(), "404": error_ref() }),
                ),
            },
            "/api/saas/custody/signers/{id}/activate": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "post": operation("saas.activateSigner", "Activate a signer. Fails closed if provider not configured; no local fallback when remote configured.", &["custody"], true, json!(null), guarded_responses("Activated.", json!({ "type": "object" }))),
            },
            "/api/saas/custody/signers/{id}": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "get": operation("saas.getSigner", "Get signer public view (address/status/capabilities, never private key).", &["custody"], true, json!(null), guarded_responses("Signer view.", json!({ "type": "object" }))),
            },

            // NOTE: `/api/saas/custody/signers/{id}/resolve` is declared in
            // `crate::api::openapi_custody` (typed `SignerView`, plus the
            // 409 "not active" and 422 "provider unavailable, fail closed"
            // cases). Merged in, not duplicated here.
            "/api/saas/organizations/{id}/lifecycle": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "get": operation("saas.getLifecycleStatus", "Inspect tenant lifecycle status (suspend/close/retention).", &["lifecycle"], true, json!(null), guarded_responses("Lifecycle status.", json!({ "type": "object" }))),
            },
            "/api/saas/organizations/{id}/close": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "post": operation("saas.requestClose", "Request tenant close. Trading disabled, credentials revoked, custody revoked, retention scheduled. Restorable only via retention policy, not via race.", &["lifecycle"], true, json!({ "type": "object", "properties": { "reason": { "type": "string" } } }), json!({ "202": json_response("Close accepted.", json!({ "type": "object" })), "403": error_ref(), "409": error_ref() })),
            },
            "/api/saas/lifecycle/jobs/{id}/advance": {
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }],
                "post": operation("saas.advanceLifecycleJob", "Advance deprovisioning job phase. Restart-safe and idempotent.", &["lifecycle"], true, json!(null), guarded_responses("Advanced.", json!({ "type": "object" }))),
            },
        },
    });

    doc
}

/// Fold the per-surface contract fragments into the document.
///
/// These four modules are the authoritative description of the billing,
/// commercial, custody and ops surfaces. Before this merge they were
/// dead code: declared in `crate::api`, compiled, and never called, so
/// the served contract silently omitted every path they describe.
///
/// Conflict policy is FAIL LOUD, not last-writer-wins. Two fragments
/// claiming the same path or the same schema name means two sources of
/// truth for one endpoint; silently keeping one of them is how a
/// published contract starts lying. The panic fires in
/// `document()`, which every request to `/api/saas/openapi.json` and the
/// artifact test both call, so a conflict cannot reach a release.
fn merge_api_fragments(doc: &mut Value) {
    use crate::api::{
        openapi_billing, openapi_commercial, openapi_custody, openapi_ops, openapi_product,
    };
    use crate::{openapi_control_plane_surface, openapi_team_security, openapi_trading_data_plane};

    let fragments: [(&str, Value, Value); 8] = [
        (
            "billing",
            openapi_billing::billing_paths(),
            openapi_billing::billing_schemas(),
        ),
        (
            "commercial",
            openapi_commercial::commercial_paths(),
            openapi_commercial::commercial_schemas(),
        ),
        (
            "custody",
            openapi_custody::custody_paths(),
            openapi_custody::custody_schemas(),
        ),
        ("ops", openapi_ops::paths(), openapi_ops::schemas()),
        (
            "trading_data_plane",
            openapi_trading_data_plane::trading_data_plane_paths(),
            openapi_trading_data_plane::trading_data_plane_schemas(),
        ),
        (
            "team_security",
            openapi_team_security::team_security_paths(),
            openapi_team_security::team_security_schemas(),
        ),
        (
            "product",
            openapi_product::product_paths(),
            openapi_product::product_schemas(),
        ),
        (
            "control_plane_surface",
            openapi_control_plane_surface::control_plane_surface_paths(),
            openapi_control_plane_surface::control_plane_surface_schemas(),
        ),
    ];

    for (name, paths, schemas) in fragments {
        merge_object(doc, &["paths"], paths, name, "path");
        merge_object(doc, &["components", "schemas"], schemas, name, "schema");
    }

    merge_object(doc, &["paths"], operator_paths(), "operator", "path");
    merge_object(
        doc,
        &["components", "schemas"],
        operator_schemas(),
        "operator",
        "schema",
    );
}

fn operator_schemas() -> Value {
    json!({
        "ModuleStatus": {
            "type": "object",
            "required": [
                "module", "enabled", "running", "healthy", "connected",
                "events_seen", "signals_generated", "orders_sent", "orders_filled",
                "orders_failed", "orders_rejected_by_risk", "realized_pnl",
                "unrealized_pnl", "consecutive_errors"
            ],
            "properties": {
                "module": { "type": "string" },
                "enabled": { "type": "boolean" },
                "running": { "type": "boolean" },
                "healthy": { "type": "boolean" },
                "connected": { "type": "boolean" },
                "events_seen": { "type": "integer" },
                "signals_generated": { "type": "integer" },
                "orders_sent": { "type": "integer" },
                "orders_filled": { "type": "integer" },
                "orders_failed": { "type": "integer" },
                "orders_rejected_by_risk": { "type": "integer" },
                "realized_pnl": { "type": "number" },
                "unrealized_pnl": { "type": "number" },
                "consecutive_errors": { "type": "integer" },
                "last_error": { "type": ["string", "null"] },
                "last_heartbeat": { "type": ["string", "null"], "format": "date-time" },
                "last_event": { "type": ["string", "null"], "format": "date-time" },
                "detail": { "type": ["string", "null"] }
            }
        },
        "OperatorKey": {
            "type": "object",
            "required": ["label", "role", "hash"],
            "properties": {
                "label": { "type": "string" },
                "role": { "type": "string", "enum": ["owner", "operator", "readonly"] },
                "hash": { "type": "string", "description": "Key fingerprint / identifier (never secret material)." },
                "source": { "type": "string", "enum": ["runtime", "database"] },
                "enabled": { "type": "boolean" },
                "created_at": { "type": "string", "format": "date-time" },
                "last_used_at": { "type": ["string", "null"], "format": "date-time" }
            }
        },
        "AuditRecord": {
            "type": "object",
            "required": ["id", "ts", "actor", "action", "outcome", "chained"],
            "properties": {
                "id": { "type": "integer" },
                "ts": { "type": "string", "format": "date-time" },
                "actor": { "type": "string" },
                "action": { "type": "string" },
                "target": { "type": ["string", "null"] },
                "outcome": { "type": "string" },
                "detail": { "type": "object", "additionalProperties": true },
                "hash": { "type": "string" },
                "chained": { "type": "boolean" }
            }
        },
        "AuditVerifyResult": {
            "type": "object",
            "required": ["status"],
            "properties": {
                "status": { "type": "string", "enum": ["intact", "broken", "not_chained", "unknown"] },
                "entries": { "type": "integer" },
                "at_id": { "type": "integer" },
                "error": { "type": "string" }
            }
        },
        "ReconciliationFinding": {
            "type": "object",
            "required": ["finding_id", "kind", "expected", "actual", "detail"],
            "properties": {
                "finding_id": { "type": "string" },
                "kind": { "type": "string" },
                "module": { "type": ["string", "null"] },
                "venue": { "type": ["string", "null"] },
                "asset": { "type": ["string", "null"] },
                "position_id": { "type": ["string", "null"] },
                "event_id": { "type": ["string", "null"] },
                "trade_id": { "type": ["string", "null"] },
                "order_id": { "type": ["string", "null"] },
                "expected": { "type": "number" },
                "actual": { "type": "number" },
                "detail": { "type": "string" }
            }
        },
        "ExecutionRecord": {
            "type": "object",
            "required": ["intent_id", "state"],
            "properties": {
                "intent_id": { "type": "string" },
                "state": { "type": "string" },
                "signature": { "type": ["string", "null"] },
                "venue": { "type": "string" },
                "symbol": { "type": "string" },
                "side": { "type": "string" },
                "quantity": { "type": "number" },
                "price": { "type": ["number", "null"] },
                "fee": { "type": "number" },
                "attempts": { "type": "integer" },
                "error": { "type": ["string", "null"] },
                "created_at": { "type": "string", "format": "date-time" },
                "updated_at": { "type": "string", "format": "date-time" }
            }
        },
        "AccountingEventRecord": {
            "type": "object",
            "required": ["kind", "wallet", "quote_asset", "reference_id"],
            "properties": {
                "event_id": { "type": "string" },
                "kind": { "type": "string" },
                "wallet": { "type": "string" },
                "asset": { "type": "string" },
                "quote_asset": { "type": "string" },
                "side": { "type": ["string", "null"] },
                "quantity": { "type": "number" },
                "price": { "type": ["number", "null"] },
                "quote_amount": { "type": "number" },
                "fee": { "type": "number" },
                "venue": { "type": "string" },
                "strategy": { "type": "string" },
                "reference_id": { "type": "string" },
                "correlation_id": { "type": ["string", "null"] },
                "position_id": { "type": ["string", "null"] },
                "counterparty_wallet": { "type": ["string", "null"] },
                "ts": { "type": "string", "format": "date-time" },
                "detail": { "type": "string" }
            }
        }
    })
}

fn operator_paths() -> Value {
    json!({
        "/api/health": {
            "get": public_operation(
                "operator.health",
                "System health and liveness probe.",
                &["operator"],
                json!(null),
                json!({
                    "200": json_response("Service is alive.", json!({ "type": "object", "required": ["ok"], "properties": { "ok": { "type": "boolean" } } })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/mode": {
            "post": operation(
                "operator.setMode",
                "Set bot execution mode (paper, simulate, live). Live execution requires owner authorization.",
                &["operator"],
                true,
                json!({
                    "type": "object",
                    "required": ["mode"],
                    "properties": {
                        "mode": { "type": "string", "enum": ["paper", "simulate", "sim", "live"] }
                    }
                }),
                json!({
                    "200": json_response("Execution mode updated.", json!({
                        "type": "object",
                        "required": ["ok", "mode", "live_allowed"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "mode": { "type": "string" },
                            "live_allowed": { "type": "boolean" },
                            "note": { "type": "string" }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/modules": {
            "get": operation(
                "operator.listModules",
                "List runtime status and telemetry for all trading bot modules.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("List of bot module runtime statuses.", json!({
                        "type": "array",
                        "items": { "$ref": "#/components/schemas/ModuleStatus" }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/modules/{name}/enable": {
            "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }],
            "post": operation(
                "operator.enableModule",
                "Enable a specific bot module runtime.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Module enabled.", json!({
                        "type": "object",
                        "required": ["ok", "module", "enabled"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "module": { "type": "string" },
                            "enabled": { "type": "boolean" }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/modules/{name}/disable": {
            "parameters": [{ "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }],
            "post": operation(
                "operator.disableModule",
                "Disable a specific bot module runtime.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Module disabled.", json!({
                        "type": "object",
                        "required": ["ok", "module", "enabled"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "module": { "type": "string" },
                            "enabled": { "type": "boolean" }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/kill": {
            "post": operation(
                "operator.kill",
                "Engage emergency stop kill switch immediately suspending all trading.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Emergency stop engaged.", json!({
                        "type": "object",
                        "required": ["ok", "kill_switch"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "kill_switch": { "type": "boolean" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/resume": {
            "post": operation(
                "operator.resume",
                "Resume trading system and clear emergency halt.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("System resumed.", json!({
                        "type": "object",
                        "required": ["ok", "kill_switch"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "kill_switch": { "type": "boolean" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/risk/kill-switch": {
            "post": operation(
                "operator.setRiskKillSwitch",
                "Engage or release a scoped venue or strategy kill switch.",
                &["operator"],
                true,
                json!({
                    "type": "object",
                    "required": ["scope", "engaged"],
                    "properties": {
                        "scope": { "type": "string", "description": "venue:<venue> or strategy:<label>" },
                        "engaged": { "type": "boolean" },
                        "reason": { "type": "string" }
                    }
                }),
                json!({
                    "200": json_response("Kill switch state updated.", json!({
                        "type": "object",
                        "required": ["ok", "scope", "outcome", "active"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "scope": { "type": "string" },
                            "outcome": { "type": "string" },
                            "active": { "type": "boolean" }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "409": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/keys": {
            "get": operation(
                "operator.listKeys",
                "List registered operator and owner API keys without secret material.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("List of API keys.", json!({
                        "type": "object",
                        "required": ["keys"],
                        "properties": {
                            "keys": { "type": "array", "items": { "$ref": "#/components/schemas/OperatorKey" } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            ),
            "post": operation(
                "operator.addKey",
                "Register a new API key. Key secret is hashed immediately and never stored or returned in plaintext.",
                &["operator"],
                true,
                json!({
                    "type": "object",
                    "required": ["label", "key", "role"],
                    "properties": {
                        "label": { "type": "string" },
                        "key": { "type": "string", "writeOnly": true, "minLength": 24 },
                        "role": { "type": "string", "enum": ["owner", "operator", "readonly"] }
                    }
                }),
                json!({
                    "200": json_response("Key registered.", json!({
                        "type": "object",
                        "required": ["ok", "label", "role", "hash"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "label": { "type": "string" },
                            "role": { "type": "string" },
                            "hash": { "type": "string", "description": "Key fingerprint" }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/keys/{hash}": {
            "parameters": [{ "name": "hash", "in": "path", "required": true, "schema": { "type": "string" } }],
            "delete": operation(
                "operator.revokeKey",
                "Revoke an API key by hash fingerprint.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Key revoked.", json!({
                        "type": "object",
                        "required": ["ok", "revoked"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "revoked": { "type": "string" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "404": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/audit": {
            "parameters": [{ "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } }],
            "get": operation(
                "operator.listAudit",
                "List immutable audit trail records with cryptographic chaining.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Recent audit records.", json!({
                        "type": "object",
                        "required": ["durable", "count", "records"],
                        "properties": {
                            "durable": { "type": "boolean" },
                            "count": { "type": "integer" },
                            "records": { "type": "array", "items": { "$ref": "#/components/schemas/AuditRecord" } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/audit/verify": {
            "get": operation(
                "operator.verifyAudit",
                "Verify cryptographic hash integrity across the recorded audit chain.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Audit verification result.", json!({
                        "$ref": "#/components/schemas/AuditVerifyResult"
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/journal": {
            "get": operation(
                "operator.journalStatus",
                "Inspect intent journal storage sizes and disk availability.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Journal status.", json!({
                        "type": "object",
                        "required": ["available"],
                        "properties": {
                            "available": { "type": "boolean" },
                            "dir": { "type": "string" },
                            "trades_bytes": { "type": "integer" },
                            "positions_bytes": { "type": "integer" },
                            "events_bytes": { "type": "integer" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            ),
            "post": operation(
                "operator.rotateJournal",
                "Rotate and archive raw JSONL intent journals.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Journal rotation outcome.", json!({
                        "type": "object",
                        "required": ["ok", "rotated", "errors"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "rotated": { "type": "array", "items": { "type": "string" } },
                            "errors": { "type": "array", "items": { "type": "string" } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/db": {
            "get": operation(
                "operator.dbStatus",
                "Database pool statistics, migration count, and deduplication backend.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Database status.", json!({
                        "type": "object",
                        "required": ["database", "dedup_backend"],
                        "properties": {
                            "database": { "type": "string", "enum": ["connected", "disabled"] },
                            "pool": {
                                "type": "object",
                                "properties": {
                                    "size": { "type": "integer" },
                                    "idle": { "type": "integer" },
                                    "active": { "type": "integer" },
                                    "max": { "type": "integer" }
                                }
                            },
                            "migrations_applied": { "type": "integer" },
                            "dedup_backend": { "type": "string" },
                            "audit_durable": { "type": "boolean" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/config": {
            "get": operation(
                "operator.configView",
                "Secrets-redacted view of runtime bot and server configuration.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Redacted runtime configuration snapshot.", json!({
                        "type": "object",
                        "additionalProperties": true
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/reconciliation/findings": {
            "parameters": [{ "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } }],
            "get": operation(
                "operator.reconciliationFindings",
                "Recent accounting and ledger reconciliation discrepancy findings.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Reconciliation findings list.", json!({
                        "type": "object",
                        "required": ["count", "findings"],
                        "properties": {
                            "count": { "type": "integer" },
                            "findings": { "type": "array", "items": { "$ref": "#/components/schemas/ReconciliationFinding" } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/recovery/failed": {
            "get": operation(
                "operator.recoveryFailed",
                "Items in the execution dead-letter and recovery queue.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Recovery queue items.", json!({
                        "type": "object",
                        "required": ["available"],
                        "properties": {
                            "available": { "type": "boolean" },
                            "count": { "type": "integer" },
                            "items": { "type": "array", "items": { "type": "object", "additionalProperties": true } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/executions": {
            "parameters": [
                { "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } },
                { "name": "state", "in": "query", "required": false, "schema": { "type": "string" } },
                { "name": "open", "in": "query", "required": false, "schema": { "type": "boolean" } }
            ],
            "get": operation(
                "operator.listExecutions",
                "List transaction intents and attempts from the execution lifecycle ledger.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Execution ledger list.", json!({
                        "type": "object",
                        "required": ["count", "states", "executions"],
                        "properties": {
                            "count": { "type": "integer" },
                            "states": { "type": "object", "additionalProperties": { "type": "integer" } },
                            "executions": { "type": "array", "items": { "$ref": "#/components/schemas/ExecutionRecord" } }
                        }
                    })),
                    "400": error_ref(), "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/executions/{id}": {
            "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
            "get": operation(
                "operator.getExecution",
                "Inspect execution lifecycle record by intent ID or transaction signature.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Execution record.", json!({
                        "$ref": "#/components/schemas/ExecutionRecord"
                    })),
                    "401": error_ref(), "403": error_ref(), "404": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/trades": {
            "parameters": [{ "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } }],
            "get": operation(
                "operator.listTrades",
                "Recent settled trade executions across all bot modules.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Recent trades array.", json!({
                        "type": "array",
                        "items": { "type": "object", "additionalProperties": true }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/wallets": {
            "get": operation(
                "operator.listWallets",
                "List system and custody wallets stored in the persistent database.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Wallets registry list.", json!({
                        "type": "object",
                        "required": ["available"],
                        "properties": {
                            "available": { "type": "boolean" },
                            "wallets": { "type": "array", "items": { "type": "object", "additionalProperties": true } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        },
        "/api/accounting/events": {
            "parameters": [{ "name": "limit", "in": "query", "required": false, "schema": { "type": "integer" } }],
            "get": operation(
                "operator.listAccountingEvents",
                "List recent financial ledger events.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Accounting events.", json!({
                        "type": "object",
                        "required": ["count", "total", "by_kind", "pending", "events"],
                        "properties": {
                            "count": { "type": "integer" },
                            "total": { "type": "integer" },
                            "by_kind": { "type": "object", "additionalProperties": { "type": "integer" } },
                            "pending": { "type": "integer" },
                            "events": { "type": "array", "items": { "$ref": "#/components/schemas/AccountingEventRecord" } }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            ),
            "post": operation(
                "operator.postAccountingEvent",
                "Book an operator-entered financial accounting event.",
                &["operator"],
                true,
                json!({
                    "type": "object",
                    "required": ["kind", "wallet", "quote_asset", "reference_id"],
                    "properties": {
                        "kind": { "type": "string", "enum": ["deposit", "withdrawal", "transfer", "funding_adjustment", "fee", "correction"] },
                        "wallet": { "type": "string" },
                        "quote_asset": { "type": "string" },
                        "reference_id": { "type": "string" },
                        "asset": { "type": "string" },
                        "side": { "type": "string", "enum": ["buy", "sell"] },
                        "quantity": { "type": "number" },
                        "price": { "type": "number" },
                        "quote_amount": { "type": "number" },
                        "fee": { "type": "number" },
                        "correlation_id": { "type": "string" },
                        "position_id": { "type": "string" },
                        "counterparty_wallet": { "type": "string" },
                        "venue": { "type": "string" },
                        "strategy": { "type": "string" },
                        "detail": { "type": "string" }
                    }
                }),
                json!({
                    "200": json_response("Accounting event booked.", json!({
                        "type": "object",
                        "required": ["ok", "outcome", "event_id"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "outcome": { "type": "string", "enum": ["new", "duplicate"] },
                            "event_id": { "type": "string" },
                            "effect": { "type": "object", "additionalProperties": true }
                        }
                    })),
                    "400": json_response("Event rejected by ledger validation.", json!({
                        "type": "object",
                        "required": ["ok", "outcome", "reason"],
                        "properties": {
                            "ok": { "type": "boolean" },
                            "outcome": { "type": "string", "enum": ["rejected"] },
                            "reason": { "type": "string" }
                        }
                    })),
                    "401": error_ref(), "403": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/events": {
            "parameters": [{ "name": "key", "in": "query", "required": false, "schema": { "type": "string" }, "description": "Authentication key for WebSocket clients" }],
            "get": operation(
                "operator.eventsWs",
                "Live system event stream WebSocket connection.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "101": { "description": "Switching Protocols to WebSocket connection" },
                    "401": error_ref(), "429": error_ref()
                })
            )
        },
        "/api/orders/{id}": {
            "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
            "get": operation(
                "operator.getOrder",
                "Inspect order details from order management system or durable archive.",
                &["operator"],
                true,
                json!(null),
                json!({
                    "200": json_response("Order record.", json!({
                        "type": "object",
                        "additionalProperties": true
                    })),
                    "401": error_ref(), "403": error_ref(), "404": error_ref(), "429": error_ref(), "503": error_ref()
                })
            )
        }
    })
}

/// Merge `incoming` into the object at `pointer`, refusing to overwrite.
fn merge_object(doc: &mut Value, pointer: &[&str], incoming: Value, origin: &str, what: &str) {
    let Some(incoming) = incoming.as_object().cloned() else {
        // A fragment that is not an object is a programming error in the
        // fragment, not a condition to paper over.
        panic!("openapi fragment '{origin}' did not return a JSON object of {what}s");
    };

    let mut target = doc;
    for key in pointer {
        target = target
            .get_mut(*key)
            .unwrap_or_else(|| panic!("openapi document has no '{key}' object to merge into"));
    }
    let target = target
        .as_object_mut()
        .unwrap_or_else(|| panic!("openapi document '{}' is not an object", pointer.join("/")));

    for (key, value) in incoming {
        if target.contains_key(&key) {
            panic!(
                "openapi fragment '{origin}' redefines {what} '{key}', which the base \
                 document already declares — two sources of truth for one endpoint"
            );
        }
        target.insert(key, value);
    }
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
        let doc = document();
        let paths = doc["paths"].as_object().unwrap();
        for internal in ["/metrics", "/ready", "/health", "/api/metrics"] {
            assert!(
                !paths.contains_key(internal),
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
                    "saas.registerUser"
                        | "saas.login"
                        | "saas.billingWebhook"
                        | "saas.billingPaymentWebhook"
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
