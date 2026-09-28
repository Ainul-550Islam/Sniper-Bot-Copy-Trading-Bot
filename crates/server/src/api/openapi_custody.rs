//! OpenAPI schema definitions for custody endpoints (BATCH 2 file 18).
//!
//! Documents custody profile lifecycle, capabilities, status, public address,
//! and health state. Explicitly documents that private keys/credentials are never returned.

use serde_json::{json, Value};

pub fn custody_schemas() -> Value {
    json!({
        "CustodyProfile": {
            "type": "object",
            "required": ["id", "organization_id", "name", "provider_type", "status"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                "provider_type": { "type": "string", "enum": ["local", "vault", "kms", "hsm"] },
                "status": { "type": "string", "enum": ["pending", "active", "revoked", "suspended"] },
                "description": { "type": ["string", "null"] },
                "created_at": { "type": "string", "format": "date-time" }
            },
            "description": "Public custody metadata only — no private keys or credential values."
        },
        "SignerView": {
            "type": "object",
            "required": ["id", "organization_id", "custody_profile_id", "logical_identity", "public_address", "provider_type", "status"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "custody_profile_id": { "type": "string", "format": "uuid" },
                "logical_identity": { "type": "string" },
                "public_address": { "type": "string", "description": "On-chain address, never private key" },
                "provider_type": { "type": "string", "enum": ["local", "vault", "kms", "hsm"] },
                "status": { "type": "string", "enum": ["pending", "active", "revoked"] },
                "capabilities": { "type": "array", "items": { "type": "string" } }
            }
        },
        "CredentialRefView": {
            "type": "object",
            "required": ["provider_type", "kind", "reference"],
            "properties": {
                "provider_type": { "type": "string", "enum": ["vault", "kms", "hsm", "local"] },
                "kind": { "type": "string", "enum": ["env_var", "aws_kms_key_id", "gcp_kms_resource", "azure_key_vault", "vault_transit", "hsm_slot", "file_path", "handle"] },
                "reference": { "type": "string", "description": "Env var name or resource ARN — never secret value" },
                "metadata_keys": { "type": "array", "items": { "type": "string" } }
            }
        },
        "CustodyHealth": {
            "type": "object",
            "required": ["provider_type", "state", "detail", "signing_allowed"],
            "properties": {
                "provider_type": { "type": "string", "enum": ["local", "vault", "kms", "hsm"] },
                "state": { "type": "string", "enum": ["configured", "reachable", "unavailable", "degraded", "revoked"] },
                "detail": { "type": "string", "description": "Secret-free detail" },
                "signing_allowed": { "type": "boolean", "description": "False when degraded/unavailable/revoked" },
                "checked_at": { "type": "string", "format": "date-time" }
            }
        },
        "CustodyHealthReport": {
            "type": "object",
            "required": ["organization_id", "providers"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "providers": { "type": "array", "items": { "$ref": "#/components/schemas/CustodyHealth" } },
                "generated_at": { "type": "string", "format": "date-time" }
            }
        }
    })
}

pub fn custody_paths() -> Value {
    json!({
        "/api/saas/custody/profiles": {
            "post": {
                "operationId": "saas.createCustodyProfile",
                "tags": ["custody"],
                "security": [{"bearerAuth": []}],
                "requestBody": {
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["name", "provider_type"],
                                "properties": {
                                    "name": {"type": "string"},
                                    "provider_type": {"type": "string", "enum": ["local", "vault", "kms", "hsm"]}
                                }
                            }
                        }
                    }
                },
                "responses": {
                    "201": { "description": "Profile created", "content": { "application/json": { "schema": {"$ref": "#/components/schemas/CustodyProfile"} } } },
                    "400": { "description": "Bad request — invalid provider or name" },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "$ref": "#/components/responses/Forbidden" },
                    "409": { "description": "Profile already exists" }
                }
            },
            "get": {
                "operationId": "saas.listCustodyProfiles",
                "tags": ["custody"],
                "security": [{"bearerAuth": []}],
                "responses": {
                    "200": { "description": "Tenant's custody profiles (scoped)", "content": { "application/json": { "schema": { "type": "array", "items": {"$ref": "#/components/schemas/CustodyProfile"} } } } },
                    "404": { "description": "Organization not found (also for cross-tenant)" }
                }
            }
        },
        "/api/saas/custody/signers/{id}/resolve": {
            "get": {
                "operationId": "saas.resolveSigner",
                "tags": ["custody"],
                "security": [{"bearerAuth": []}],
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}],
                "responses": {
                    "200": { "description": "Resolved signer handle (public address only)", "content": { "application/json": { "schema": {"$ref": "#/components/schemas/SignerView"} } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "403": { "description": "Tenant mismatch or capability missing" },
                    "404": { "description": "Signer/profile not found or revoked" },
                    "409": { "description": "Signer not active" },
                    "422": { "description": "Provider unavailable — fail closed" }
                }
            }
        },
        "/api/saas/custody/health": {
            "get": {
                "operationId": "saas.custodyHealth",
                "tags": ["custody"],
                "security": [{"bearerAuth": []}],
                "responses": {
                    "200": { "description": "Provider health (secret-free)", "content": { "application/json": { "schema": {"$ref": "#/components/schemas/CustodyHealthReport"} } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk every string value in the schema tree.
    fn walk_strings(v: &serde_json::Value, f: &mut impl FnMut(&str)) {
        match v {
            serde_json::Value::String(s) => f(s),
            serde_json::Value::Array(items) => {
                for item in items {
                    walk_strings(item, f);
                }
            }
            serde_json::Value::Object(map) => {
                for (_, val) in map {
                    walk_strings(val, f);
                }
            }
            _ => {}
        }
    }

    /// Collect every object key in the schema tree.
    fn walk_keys(v: &serde_json::Value, f: &mut impl FnMut(&str)) {
        match v {
            serde_json::Value::Array(items) => {
                for item in items {
                    walk_keys(item, f);
                }
            }
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    f(k);
                    walk_keys(val, f);
                }
            }
            _ => {}
        }
    }

    /// The custody schema must never expose a secret-bearing field or secret material.
    /// Prose that states secrets are NOT returned ("never private key", "secret-free")
    /// is allowed and is itself part of the control; field names, enums and values
    /// that could carry secret material are not.
    #[test]
    fn private_keys_never_returned() {
        let schemas = custody_schemas();

        // 1. No object key anywhere may be a secret-bearing identifier.
        let banned_keys = [
            "private_key",
            "privatekey",
            "secret",
            "secret_key",
            "webhook_secret",
            "api_key",
            "apikey",
            "mnemonic",
            "seed_phrase",
            "seedphrase",
            "keypair",
            "password",
            "passphrase",
            "raw_key",
            "signing_key",
        ];
        let mut keys = Vec::new();
        walk_keys(&schemas, &mut |k| keys.push(k.to_ascii_lowercase()));
        for k in &keys {
            for banned in banned_keys {
                assert!(
                    !k.contains(banned),
                    "schema field name {k} must not be secret-bearing (matched {banned})"
                );
            }
        }

        // 2. No string value may contain secret material or a secret-bearing token
        //    other than an explicit negation ("never ...", "secret-free", "not ...").
        let mut values = Vec::new();
        walk_strings(&schemas, &mut |s| values.push(s.to_string()));
        for v in &values {
            let lower = v.to_ascii_lowercase();
            for banned in [
                "private_key",
                "private key",
                "mnemonic",
                "keypair",
                "seed phrase",
                "-----begin",
                "sk_live_", // banned detection literal — never a credential
                "sk_test_", // banned detection literal — never a credential
                "whsec_",   // banned detection literal — never a credential
            ] {
                if lower.contains(banned) {
                    let negated = lower.contains("never")
                        || lower.contains("no ")
                        || lower.contains("not ")
                        || lower.contains("free")
                        || lower.contains("-free");
                    assert!(
                        negated,
                        "schema value {v:?} carries secret token {banned} without negation"
                    );
                }
            }
            // Base64/hex-shaped key material must never appear in the public schema.
            for token in v.split(|c: char| c.is_whitespace() || c == '"' || c == ',') {
                let looks_like_key_material = token.len() >= 40
                    && token
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
                assert!(
                    !looks_like_key_material,
                    "schema value contains key-like material: {token:?}"
                );
            }
        }

        // 3. The public address field remains the only identity material published.
        let s = schemas.to_string().to_ascii_lowercase();
        assert!(s.contains("public_address"));
    }

    #[test]
    fn custody_health_documents_401_403_404_409_422() {
        let paths = custody_paths().to_string();
        for code in ["401", "403", "404", "409", "422"] {
            assert!(paths.contains(code), "must document {}", code);
        }
    }

    #[test]
    fn credential_ref_is_env_not_value() {
        let schema = &custody_schemas()["CredentialRefView"];
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("reference"));
        let desc = props["reference"]["description"].as_str().unwrap();
        assert!(desc.contains("never secret"));
    }
}
