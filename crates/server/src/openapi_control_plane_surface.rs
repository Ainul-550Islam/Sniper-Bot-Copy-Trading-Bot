//! OpenAPI v3 paths for the full authenticated control-plane surface (Part 5).
//!
//! Before this fragment existed, `openapi/openapi.json` documented only the
//! original TASK 7B core plus the Part 2 billing/commercial/custody/ops/
//! product/team-security/trading-data-plane fragments — while the control-plane
//! web app (`apps/control-plane`) and this router agreed on ~70 more routes
//! that were nowhere in the contract. `scripts/check-openapi-coverage.sh` and
//! the frontend `api-contract` test both fail on that drift; this fragment is
//! the remediation.
//!
//! Conventions kept from the sibling fragments:
//! - pure `json!` literals only (no Rust expressions inside), so the exported
//!   artifact can be regenerated deterministically;
//! - response bodies are documented as `object` unless the handler's shape is
//!   already published elsewhere — this file documents ROUTING, AUTHN/AUTHZ
//!   expectations and intent, and never invents field lists;
//! - `401` references the shared `Unauthorized` response; platform-admin-only
//!   operations also reference `Forbidden`.

use serde_json::{json, Value};

/// Paths contributed by this fragment.
#[rustfmt::skip]
pub fn control_plane_surface_paths() -> Value {
    json!({
        "/api/saas/activity": {
            "get": {
                "tags": ["events"],
                "operationId": "saas.activity",
                "description": "Tenant activity feed (audit-derived events for the caller's organization).",
                "responses": {
                    "200": { "description": "The organization's recent activity page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/alerts": {
            "get": {
                "tags": ["events"],
                "operationId": "saas.listAlerts",
                "description": "Recent tenant alerts (newest first, at most 200) and the unacknowledged count for the caller's organization.",
                "responses": {
                    "200": { "description": "Recent tenant alerts and the unacknowledged count.", "content": { "application/json": { "schema": { "type": "object", "required": ["organization_id", "items", "unacknowledged_count"], "properties": { "organization_id": { "type": "string", "format": "uuid" }, "items": { "type": "array", "items": { "$ref": "#/components/schemas/AlertItem" } }, "unacknowledged_count": { "type": "integer", "minimum": 0 } } } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/notifications/preferences": {
            "get": {
                "tags": ["events"],
                "operationId": "saas.notificationPreferences",
                "description": "The caller's notification preferences.",
                "responses": {
                    "200": { "description": "Preferences.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/portfolio": {
            "get": {
                "tags": ["reports"],
                "operationId": "saas.portfolio",
                "description": "Read-only portfolio roll-up for the caller's organization, assembled from the trading data plane.",
                "responses": {
                    "200": { "description": "The portfolio roll-up.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PortfolioSummary" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/pricing": {
            "get": {
                "tags": ["billing"],
                "operationId": "saas.pricing",
                "description": "The public plan/pricing catalogue rendered on the pricing page. Read-only; contains no tenant data.",
                "responses": {
                    "200": { "description": "Plan catalogue.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            }
        },
        "/api/saas/risk-dashboard": {
            "get": {
                "tags": ["reports"],
                "operationId": "saas.riskDashboard",
                "description": "Organization risk dashboard: gate states, kill-switch position and recent risk events.",
                "responses": {
                    "200": { "description": "Risk dashboard payload.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RiskDashboardState" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/risk-dashboard/kill-switch": {
            "post": {
                "tags": ["reports"],
                "operationId": "saas.killSwitch",
                "description": "Arm or disarm the organization kill switch. Refusal kinds are stable (see kill_switch_flow.rs); the response field is `outcome`.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The new kill-switch state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/status": {
            "get": {
                "tags": ["contract"],
                "operationId": "saas.status",
                "description": "Control-plane status: version, feature flags and dependency health, with secret-free redaction.",
                "responses": {
                    "200": { "description": "Status payload.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            }
        },
        "/api/saas/support/tickets": {
            "get": {
                "tags": ["events"],
                "operationId": "saas.listSupportTickets",
                "description": "List the caller organization's support tickets.",
                "responses": {
                    "200": { "description": "The ticket list.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "post": {
                "tags": ["events"],
                "operationId": "saas.createSupportTicket",
                "description": "Open a support ticket for the caller's organization.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "201": { "description": "The ticket was created.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/alerts/{id}/ack": {
            "post": {
                "tags": ["events"],
                "operationId": "saas.acknowledgeAlert",
                "description": "Acknowledge one alert by id. Idempotent: re-acknowledging an acknowledged alert is a no-op success.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The alert is acknowledged.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/audit/export": {
            "get": {
                "tags": ["exports"],
                "operationId": "saas.auditExport",
                "description": "Download the caller organization's audit trail export. Redacted: secrets and token material never appear in exports.",
                "responses": {
                    "200": { "description": "The export payload.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/billing/reconcile/{id}": {
            "get": {
                "tags": ["billing"],
                "operationId": "saas.billingReconcileFor",
                "description": "Platform-admin read of one organization's billing reconciliation state. The id-less variant (`/api/saas/billing/reconcile`) serves the caller's own organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Reconciliation state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/billing/status/{id}": {
            "get": {
                "tags": ["billing"],
                "operationId": "saas.billingStatusFor",
                "description": "Platform-admin read of one organization's billing status. The id-less variant serves the caller's own organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Billing status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/commercial/state/{id}": {
            "get": {
                "tags": ["billing"],
                "operationId": "saas.commercialStateFor",
                "description": "Platform-admin read of one organization's commercial state (plan, entitlements, kill-switch flags). The id-less variant serves the caller's own organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Commercial state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/health/{id}": {
            "get": {
                "tags": ["custody"],
                "operationId": "saas.custodyHealthFor",
                "description": "Platform-admin read of one organization's custody health. The id-less variant serves the caller's own organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Custody health.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/profiles/{id}/revoke": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.revokeCustodyProfile",
                "description": "Revoke one custody profile. Irreversible for the revoked profile; audited.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The profile is revoked.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/profiles/{id}/signers": {
            "get": {
                "tags": ["custody"],
                "operationId": "saas.listCustodyProfileSigners",
                "description": "List the signers attached to one custody profile. Public key material is returned; private material never is.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The profile's signers.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/custody/rotations": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.createCustodyRotation",
                "description": "Open a signer rotation for a custody profile (old signer -> new signer). The rotation stays pending until activated.",
                "requestBody": {
                    "required": true,
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["profile_id", "old_signer_id", "new_signer_id"],
                        "properties": {
                            "profile_id": { "type": "string", "format": "uuid" },
                            "old_signer_id": { "type": "string", "format": "uuid" },
                            "new_signer_id": { "type": "string", "format": "uuid" }
                        }
                    } } }
                },
                "responses": {
                    "201": { "description": "The rotation was created.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/rotations/{id}": {
            "get": {
                "tags": ["custody"],
                "operationId": "saas.getCustodyRotation",
                "description": "Read one rotation's state machine (pending / activated / revoked).",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The rotation.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/custody/rotations/{id}/activate": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.activateCustodyRotation",
                "description": "Activate a pending rotation: the new signer becomes the profile's signer.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The rotation is active.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/rotations/{id}/revoke": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.revokeCustodyRotation",
                "description": "Revoke a pending rotation before it activates.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The rotation is revoked.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/signers/{id}/capabilities": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.setSignerCapabilities",
                "description": "Update what a signer is allowed to do (per-capability flags).",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "Capabilities updated.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/custody/signers/{id}/revoke": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.revokeSigner",
                "description": "Revoke one signer. The signer stops signing immediately; audited.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The signer is revoked.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/data-lifecycle/{id}/disconnect": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.disconnectIntegration",
                "description": "Disconnect one integration for the caller's organization and stop any jobs that use it.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The integration is disconnected.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/data-lifecycle/{id}/revoke-credentials": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.revokeIntegrationCredentials",
                "description": "Revoke the stored credentials of one integration without deleting its history.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Credentials revoked.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/data-lifecycle/{id}/revoke-sessions": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.revokeIntegrationSessions",
                "description": "Revoke live sessions that were established through one integration.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Sessions revoked.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/data-lifecycle/{id}/schedule-retention": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.scheduleRetention",
                "description": "Schedule data-retention processing for one integration's stored data.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Retention scheduled.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/openapi.json": {
            "get": {
                "tags": ["contract"],
                "operationId": "saas.openapiDocument",
                "security": [],
                "description": "The OpenAPI document itself, served unauthenticated: a contract is only useful if a buyer can read it before buying. The committed artifact at openapi/openapi.json is byte-identical (enforced by crates/server/tests/openapi_artifact.rs).",
                "responses": {
                    "200": { "description": "The OpenAPI 3.1 document.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            }
        },
        "/api/saas/organizations/{id}/resume": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.resumeOrganization",
                "description": "Platform-admin only: resume a suspended organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The organization is active again.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/organizations/{id}/suspend": {
            "post": {
                "tags": ["lifecycle"],
                "operationId": "saas.suspendOrganization",
                "description": "Platform-admin only: suspend an organization. Authenticated traffic is refused while suspended; the kill-switch test suite covers the flow.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The organization is suspended.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/team/members/{id}": {
            "delete": {
                "tags": ["organizations"],
                "operationId": "saas.removeTeamMember",
                "description": "Remove one member from the caller's organization. Scoped to the caller's tenant; naming another tenant's member is refused without revealing existence.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The member is removed.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            },
            "patch": {
                "tags": ["organizations"],
                "operationId": "saas.updateTeamMemberRole",
                "description": "Change one member's role. Role-assignment limits are enforced server-side (a role above the caller's own is `role_not_assignable_by_tenant`).",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "requestBody": {
                    "required": true,
                    "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["role"],
                        "properties": { "role": { "type": "string" } }
                    } } }
                },
                "responses": {
                    "200": { "description": "The member's new role.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/usage/limits/{id}": {
            "get": {
                "tags": ["billing"],
                "operationId": "saas.usageLimitsFor",
                "description": "Platform-admin read of one organization's usage limits. The id-less variant serves the caller's own organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "Usage limits.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },

        "/api/tenant/analytics": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.analytics",
                "description": "Aggregated analytics for the tenant's trading activity (supports a `timeframe` query parameter).",
                "responses": {
                    "200": { "description": "Analytics summary.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/backtests/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getBacktest",
                "description": "One backtest run including `result_json`. Synthetic-price runs are labelled `synthetic` in the stored result; the API never presents them as live performance.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The backtest run.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/balances": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.balances",
                "description": "Wallet balances visible to the tenant.",
                "responses": {
                    "200": { "description": "Balances.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/bots": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listBots",
                "description": "Runtime state of every module family for the tenant.",
                "responses": {
                    "200": { "description": "Module family runtimes.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/bots/{module}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getBot",
                "description": "One module family's runtime state.",
                "parameters": [ { "name": "module", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The module runtime.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/copy/config": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getCopyConfig",
                "description": "Current copy-trading configuration for the tenant.",
                "responses": {
                    "200": { "description": "Copy config.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "put": {
                "tags": ["tenant"],
                "operationId": "tenant.putCopyConfig",
                "description": "Replace the copy-trading configuration. Writes are versioned and audited; a fenced paper mode applies when the tenant is not funded for live.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The stored configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/copy/controls": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.copyControls",
                "description": "Copy-trading control actions (start/stop/pause). Refusals use stable error kinds.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The action outcome.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/copy/leaders": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listCopyLeaders",
                "description": "Discoverable copy-trading leader wallets.",
                "responses": {
                    "200": { "description": "Leader list.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/copy/leaders/{address}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getCopyLeader",
                "description": "One leader wallet's copyable profile.",
                "parameters": [ { "name": "address", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The leader profile.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/copy/links": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.copyLinks",
                "description": "Open copy links (leader -> tenant follower relationships).",
                "responses": {
                    "200": { "description": "Open links.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/copy/status": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.copyStatus",
                "description": "Copy-trading module status.",
                "responses": {
                    "200": { "description": "Module status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/executions": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listExecutions",
                "description": "Execution history (supports `since`, `until`, `order_id` query parameters).",
                "responses": {
                    "200": { "description": "Executions page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/executions/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getExecution",
                "description": "One execution by id.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The execution.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/integrations": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listIntegrations",
                "description": "Integration states for the tenant (Telegram, Polymarket, exchanges).",
                "responses": {
                    "200": { "description": "Integration list.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/markets/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getMarket",
                "description": "One discovered market by id. Returns 503 `market_data_unavailable` (never fabricated rows) when the live feeds are dark.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The market.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/onboarding": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.onboardingState",
                "description": "Tenant onboarding checklist state.",
                "responses": {
                    "200": { "description": "Onboarding state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/onboarding/complete": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.completeOnboardingStep",
                "description": "Mark one onboarding step complete. Idempotent per step.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "Updated onboarding state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/orders": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listOrders",
                "description": "Order history (supports `limit` and `cursor` query parameters).",
                "responses": {
                    "200": { "description": "Orders page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/orders/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getOrder",
                "description": "One order by id.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The order.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/orders/{id}/cancel": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.cancelOrder",
                "description": "Request cancellation of one order. Refusal kinds are stable; a cancel of an already-terminal order is refused, not faked.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The cancellation outcome.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/limit-orders": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listLimitOrders",
                "description": "This organization's limit orders, newest first. Filter with `status`; page with `limit` (1-200) and `before` (RFC 3339 created_at, exclusive).",
                "parameters": [
                    { "name": "status", "in": "query", "required": false, "schema": { "type": "string", "enum": ["active", "triggered", "cancelled", "expired"] } },
                    { "name": "limit", "in": "query", "required": false, "schema": { "type": "integer", "minimum": 1, "maximum": 200 } },
                    { "name": "before", "in": "query", "required": false, "schema": { "type": "string", "format": "date-time" } }
                ],
                "responses": {
                    "200": { "description": "A page of orders.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "422": { "description": "Invalid filter.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "503": { "description": "Storage unavailable. Never returns fabricated orders.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            },
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.createLimitOrder",
                "description": "Create an ACTIVE limit order. Records intent only; execution is performed by the limit-order worker when the trigger is met.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "201": { "description": "Order created.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "422": { "description": "Validation failed (side, trigger, mint, price, amount or expiry).", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "503": { "description": "Storage unavailable.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            }
        },
        "/api/tenant/limit-orders/{id}/cancel": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.cancelLimitOrder",
                "description": "Cancel an ACTIVE order that no worker is currently executing. Refusals are explicit: 409 when already terminal or being triggered, 404 when not found in this organization.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "Cancelled.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "Not found in this organization.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "409": { "description": "Not active, or being triggered.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "503": { "description": "Storage unavailable.", "content": { "application/json": { "schema": { "type": "object" } } } }
                }
            }
        },
        "/api/tenant/dca-schedules": {
                "get": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.listDcaSchedules",
                        "description": "This organization's DCA schedules, newest first. Filter with `status`; `limit` 1-200.",
                        "responses": {
                                "200": {
                                        "description": "OK.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "422": {
                                        "description": "Invalid filter.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "parameters": [
                                {
                                        "name": "status",
                                        "in": "query",
                                        "required": false,
                                        "schema": {
                                                "type": "string",
                                                "enum": [
                                                        "active",
                                                        "completed",
                                                        "paused",
                                                        "cancelled"
                                                ]
                                        }
                                },
                                {
                                        "name": "limit",
                                        "in": "query",
                                        "required": false,
                                        "schema": {
                                                "type": "integer",
                                                "minimum": 1,
                                                "maximum": 200
                                        }
                                }
                        ]
                },
                "post": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.createDcaSchedule",
                        "description": "Create a DCA schedule with a hard SOL budget. Unlimited schedules are refused. Records intent per interval slot; never a fill.",
                        "responses": {
                                "201": {
                                        "description": "Created.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "403": {
                                        "$ref": "#/components/responses/Forbidden"
                                },
                                "422": {
                                        "description": "Validation failed.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "requestBody": {
                                "required": true,
                                "content": {
                                        "application/json": {
                                                "schema": {
                                                        "type": "object"
                                                }
                                        }
                                }
                        }
                }
        },
        "/api/tenant/dca-schedules/{id}/runs": {
                "get": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.listDcaRuns",
                        "description": "Run ledger for one schedule (intent rows, newest first). Foreign ids return 404.",
                        "responses": {
                                "200": {
                                        "description": "OK.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "404": {
                                        "description": "Not found in this organization.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "parameters": [
                                {
                                        "name": "id",
                                        "in": "path",
                                        "required": true,
                                        "schema": {
                                                "type": "string"
                                        }
                                }
                        ]
                }
        },
        "/api/tenant/dca-schedules/{id}/pause": {
                "post": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.pauseDcaSchedule",
                        "description": "Pause an ACTIVE schedule. No runs are recorded while paused.",
                        "responses": {
                                "200": {
                                        "description": "OK.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "403": {
                                        "$ref": "#/components/responses/Forbidden"
                                },
                                "404": {
                                        "description": "Not found in this organization.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "409": {
                                        "description": "Invalid transition, or a worker is recording a run.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "parameters": [
                                {
                                        "name": "id",
                                        "in": "path",
                                        "required": true,
                                        "schema": {
                                                "type": "string"
                                        }
                                }
                        ]
                }
        },
        "/api/tenant/dca-schedules/{id}/resume": {
                "post": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.resumeDcaSchedule",
                        "description": "Resume a PAUSED schedule. The clock is re-anchored to now + interval, so missed slots are never replayed.",
                        "responses": {
                                "200": {
                                        "description": "OK.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "403": {
                                        "$ref": "#/components/responses/Forbidden"
                                },
                                "404": {
                                        "description": "Not found in this organization.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "409": {
                                        "description": "Invalid transition, or a worker is recording a run.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "parameters": [
                                {
                                        "name": "id",
                                        "in": "path",
                                        "required": true,
                                        "schema": {
                                                "type": "string"
                                        }
                                }
                        ]
                }
        },
        "/api/tenant/dca-schedules/{id}/cancel": {
                "post": {
                        "tags": [
                                "tenant"
                        ],
                        "operationId": "tenant.cancelDcaSchedule",
                        "description": "Cancel an ACTIVE or PAUSED schedule. Refused with 409 while a worker is recording a run.",
                        "responses": {
                                "200": {
                                        "description": "OK.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "401": {
                                        "$ref": "#/components/responses/Unauthorized"
                                },
                                "403": {
                                        "$ref": "#/components/responses/Forbidden"
                                },
                                "404": {
                                        "description": "Not found in this organization.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "409": {
                                        "description": "Invalid transition, or a worker is recording a run.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                },
                                "503": {
                                        "description": "Storage unavailable.",
                                        "content": {
                                                "application/json": {
                                                        "schema": {
                                                                "type": "object"
                                                        }
                                                }
                                        }
                                }
                        },
                        "parameters": [
                                {
                                        "name": "id",
                                        "in": "path",
                                        "required": true,
                                        "schema": {
                                                "type": "string"
                                        }
                                }
                        ]
                }
        },
        "/api/tenant/polymarket/config": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getPolymarketConfig",
                "description": "Polymarket module configuration for the tenant.",
                "responses": {
                    "200": { "description": "Configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "put": {
                "tags": ["tenant"],
                "operationId": "tenant.putPolymarketConfig",
                "description": "Replace the Polymarket module configuration. Versioned and audited.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The stored configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/polymarket/controls": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.polymarketControls",
                "description": "Polymarket module control actions.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The action outcome.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/polymarket/fills": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listPolymarketFills",
                "description": "Polymarket fills history (supports `since` and `until` query parameters).",
                "responses": {
                    "200": { "description": "Fills page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/polymarket/orders": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listPolymarketOrders",
                "description": "Polymarket orders (supports `limit` and `cursor` query parameters).",
                "responses": {
                    "200": { "description": "Orders page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/polymarket/orders/{venue_order_id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getPolymarketOrder",
                "description": "One Polymarket order by venue order id.",
                "parameters": [ { "name": "venue_order_id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The order.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/polymarket/reconciliation": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.polymarketReconciliation",
                "description": "Venue-vs-local reconciliation report for Polymarket positions.",
                "responses": {
                    "200": { "description": "Reconciliation report.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/polymarket/status": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.polymarketStatus",
                "description": "Polymarket module status.",
                "responses": {
                    "200": { "description": "Module status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/positions": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listPositions",
                "description": "Positions (supports `limit` and `cursor` query parameters).",
                "responses": {
                    "200": { "description": "Positions page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/positions/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getPosition",
                "description": "One position by id.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The position.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/recovery/intents": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.recoveryIntents",
                "description": "Stuck-intent recovery queue state.",
                "responses": {
                    "200": { "description": "Recovery intents.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/recovery/sweep": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.recoverySweep",
                "description": "Dust/sweep recovery status for tenant wallets.",
                "responses": {
                    "200": { "description": "Sweep status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/reports/pnl": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.pnlReport",
                "description": "Realized/unrealized PnL report assembled from execution history. No projections, no synthetic rows.",
                "responses": {
                    "200": { "description": "PnL report.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/reports/summary": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.summaryReport",
                "description": "Tenant activity summary report.",
                "responses": {
                    "200": { "description": "Summary report.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/sniper/config": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getSniperConfig",
                "description": "Sniper module configuration for the tenant.",
                "responses": {
                    "200": { "description": "Configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "put": {
                "tags": ["tenant"],
                "operationId": "tenant.putSniperConfig",
                "description": "Replace the sniper module configuration. Versioned and audited.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The stored configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/sniper/controls": {
            "post": {
                "tags": ["tenant"],
                "operationId": "tenant.sniperControls",
                "description": "Sniper module control actions.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The action outcome.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/sniper/status": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.sniperStatus",
                "description": "Sniper module status.",
                "responses": {
                    "200": { "description": "Module status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/strategies/{id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getStrategy",
                "description": "One strategy record by id.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The strategy.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            },
            "put": {
                "tags": ["tenant"],
                "operationId": "tenant.updateStrategy",
                "description": "Update name/description/status/config of one strategy. Re-activating goes through the versioned-config activation gate (strategy_lifecycle.rs); refusals return 409 `strategy_activation_denied`.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The updated strategy.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            },
            "delete": {
                "tags": ["tenant"],
                "operationId": "tenant.archiveStrategy",
                "description": "Archive one strategy (soft delete). Archived strategies cannot be re-activated or backtested.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The archived strategy.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/telegram/binding": {
            "put": {
                "tags": ["tenant"],
                "operationId": "tenant.bindTelegram",
                "description": "Bind the tenant's Telegram chat to the trading bridge.",
                "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                "responses": {
                    "200": { "description": "The binding.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            },
            "delete": {
                "tags": ["tenant"],
                "operationId": "tenant.unbindTelegram",
                "description": "Remove the tenant's Telegram binding.",
                "responses": {
                    "200": { "description": "The binding is removed.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/tenant/telegram/status": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.telegramStatus",
                "description": "Telegram bridge status for the tenant.",
                "responses": {
                    "200": { "description": "Bridge status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/trades": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listTrades",
                "description": "Trade history across positions.",
                "responses": {
                    "200": { "description": "Trades page.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/trades/{position_id}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.listPositionTrades",
                "description": "Trades belonging to one position.",
                "parameters": [ { "name": "position_id", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The position's trades.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/tenant/transactions/{signature}": {
            "get": {
                "tags": ["tenant"],
                "operationId": "tenant.getTransaction",
                "description": "One on-chain transaction by signature, as observed by the tenant's execution path.",
                "parameters": [ { "name": "signature", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The transaction record.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },

        "/api/accounting/findings": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.accountingFindings",
                "description": "Read-only accounting reconciliation findings.",
                "responses": {
                    "200": { "description": "Findings.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/accounting/portfolio": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.accountingPortfolio",
                "description": "Read-only accounting view of the portfolio.",
                "responses": {
                    "200": { "description": "Portfolio accounting.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/ha": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.ha",
                "description": "High-availability state: leader, replicas, failover readiness.",
                "responses": {
                    "200": { "description": "HA state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/orders": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.listOrders",
                "description": "Read-only operator view of all orders.",
                "responses": {
                    "200": { "description": "Orders.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/positions": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.listPositions",
                "description": "Read-only operator view of all positions.",
                "responses": {
                    "200": { "description": "Positions.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/risk/global": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.globalRisk",
                "description": "Read-only global risk state (limits, gates, kill-switch position).",
                "responses": {
                    "200": { "description": "Global risk state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/status": {
            "get": {
                "tags": ["operator"],
                "operationId": "operator.status",
                "description": "Operator service status (engines, feeds, storage).",
                "responses": {
                    "200": { "description": "Status.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/referrals/codes": {
            "get": {
                "tags": ["referrals"],
                "operationId": "saas.listReferralCodes",
                "description": "List this organization's referral codes with redemption counts (migration 0049).",
                "responses": {
                    "200": { "description": "The organization's referral codes.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "post": {
                "tags": ["referrals"],
                "operationId": "saas.mintReferralCode",
                "description": "Mint a referral code for this organization. Codes are generated server-side from the OS RNG; clients never choose codes.",
                "responses": {
                    "201": { "description": "The minted code (shown once).", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/referrals/codes/{code}/disable": {
            "post": {
                "tags": ["referrals"],
                "operationId": "saas.disableReferralCode",
                "description": "Retire one of this organization's referral codes. Existing attributions are untouched (they are facts, not permissions).",
                "parameters": [ { "name": "code", "in": "path", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The code is disabled.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "No active code with that value belongs to this organization." }
                }
            }
        },
        "/api/saas/referrals/attribution": {
            "get": {
                "tags": ["referrals"],
                "operationId": "saas.referralAttribution",
                "description": "Inbound attribution (who referred this organization) and outbound list (organizations this organization referred).",
                "responses": {
                    "200": { "description": "Attribution view.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        "/api/saas/referrals/redeem": {
            "post": {
                "tags": ["referrals"],
                "operationId": "saas.redeemReferralCode",
                "description": "Attribute this organization to a referral code. Once per organization, forever (anti-churn); an organization cannot redeem its own code.",
                "responses": {
                    "201": { "description": "Attribution recorded.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "No active referral code with that value." },
                    "409": { "description": "Already attributed, or self-referral." }
                }
            }
        },
        "/api/saas/wallet-pools": {
            "get": {
                "tags": ["custody"],
                "operationId": "saas.listWalletPools",
                "description": "List this organization's wallet pools (migration 0050) with active member counts.",
                "responses": {
                    "200": { "description": "The organization's wallet pools.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            },
            "post": {
                "tags": ["custody"],
                "operationId": "saas.createWalletPool",
                "description": "Create a named pool of this organization's custody signers. allocation is round_robin or weighted_split.",
                "responses": {
                    "201": { "description": "The created pool.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "409": { "description": "A pool with that name already exists for this organization." }
                }
            }
        },
        "/api/saas/wallet-pools/{id}": {
            "get": {
                "tags": ["custody"],
                "operationId": "saas.getWalletPool",
                "description": "One wallet pool with its active members, resolved to same-org custody signers.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The pool and its members.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "404": { "description": "No pool with that id belongs to this organization." }
                }
            },
            "patch": {
                "tags": ["custody"],
                "operationId": "saas.updateWalletPool",
                "description": "Partial update of a pool: name, allocation, or status (active/disabled).",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "200": { "description": "The updated pool.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "No pool with that id belongs to this organization." }
                }
            }
        },
        "/api/saas/wallet-pools/{id}/members": {
            "post": {
                "tags": ["custody"],
                "operationId": "saas.addWalletPoolMember",
                "description": "Add one of this organization's active custody signers to the pool. Cross-tenant membership is refused in Rust and by a Postgres trigger.",
                "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                "responses": {
                    "201": { "description": "The new membership.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "No pool with that id belongs to this organization." },
                    "409": { "description": "That signer is already a member." }
                }
            }
        },
        "/api/saas/wallet-pools/{id}/members/{member_id}": {
            "delete": {
                "tags": ["custody"],
                "operationId": "saas.removeWalletPoolMember",
                "description": "Remove a member (status=removed). Immediate for NEW orders; open positions stay with their original wallet.",
                "parameters": [
                    { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
                    { "name": "member_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                ],
                "responses": {
                    "200": { "description": "The member is removed.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "No such membership in this organization." }
                }
            }
        },
        "/api/saas/sso/config": {
            "get": {
                "tags": ["security"],
                "operationId": "saas.getSsoConfig",
                "description": "This organization's SSO configuration, without secrets (client_secret_set is a boolean).",
                "responses": {
                    "200": { "description": "The SSO configuration.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "404": { "description": "SSO is not configured for this organization." }
                }
            },
            "put": {
                "tags": ["security"],
                "operationId": "saas.putSsoConfig",
                "description": "Create or replace the organization's OIDC SSO configuration (migration 0053). The client secret is encrypted at rest and never returned. role_mapping can never grant platform_admin.",
                "responses": {
                    "200": { "description": "The stored configuration (no secrets).", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            },
            "delete": {
                "tags": ["security"],
                "operationId": "saas.deleteSsoConfig",
                "description": "Remove SSO for this organization.",
                "responses": {
                    "200": { "description": "Removed.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" }
                }
            }
        },
        "/api/saas/sso/authorize": {
            "get": {
                "tags": ["security"],
                "operationId": "saas.ssoAuthorize",
                "security": [],
                "description": "Begin the OIDC authorization-code + PKCE flow for a tenant slug. Returns the IdP authorization URL and the opaque state; the front-end owns the redirect.",
                "parameters": [ { "name": "organization", "in": "query", "required": true, "schema": { "type": "string" } } ],
                "responses": {
                    "200": { "description": "The authorize URL and state.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "404": { "description": "No active SSO configuration for that organization." }
                }
            }
        },
        "/api/saas/sso/callback": {
            "post": {
                "tags": ["security"],
                "operationId": "saas.ssoCallback",
                "security": [],
                "description": "Finish the OIDC flow: one-shot state, PKCE verifier, RS256 id_token signature vs issuer JWKS, iss/aud/exp, allowed_domains, JIT provisioning, session token returned exactly once.",
                "responses": {
                    "200": { "description": "User, organization, session and the one-time session token.", "content": { "application/json": { "schema": { "type": "object" } } } },
                    "400": { "description": "Invalid state, missing email, or domain not allowed." },
                    "502": { "description": "IdP discovery, token exchange, or signature verification failed." }
                }
            }
        }
    })
}

pub fn control_plane_surface_schemas() -> Value {
    json!({})
}
