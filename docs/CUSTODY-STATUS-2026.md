# Custody Status 2026 (2026-09-30)

EVIDENCE-LEVEL: UNIT_TEST

The exact custody implementation state per provider, as of today. No
provider is described as production-live that has not been exercised
against a real backend.

## Architecture (all providers share this)

1. **Domain** (`bot-core::custody`): profiles, signer records, policy,
   resolution, health, rotation. Fail-closed: no local fallback for
   remote-bound signers, ever.
2. **Boundary** (`crates/server/src/custody/`): deployment posture
   (`provider_registry.rs`), sign request/response contracts, the
   guard-ordered sign executor (`sign_boundary.rs`), boundary health,
   audit trail. Every outcome — signed or refused — is audited.
3. **Providers**: the registry activates exactly one provider per
   deployment (`CUSTODY_PROVIDER=local|vault|kms|hsm`); remote custody
   additionally requires `LIVE_CUSTODY=1` (no silent remote activation).

## Provider status

| Provider | Implementation | Signing | Readiness state without backend | Evidence |
| --- | --- | --- | --- | --- |
| local | Adapter present; the single-operator deployment signs via the existing solana module wallet path (OPERATOR_KEY/WALLET env) — the multi-tenant boundary does not intercept that path | Refused through the multi-tenant boundary with the exact dependency named | `ready` for single-operator posture | CODE + unit tests |
| **vault** | **REAL transit-engine integration** (`crates/server/src/custody/vault/`): REST + JSON over reqwest — `sys/health`, `token/lookup-self`, `transit/keys/{key}`, `transit/sign/{key}` (ed25519). Reference-only config (VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_MOUNT/VAULT_TRANSIT_KEY); token held in a redacted wrapper, never logged | Real: signature comes only from a Vault response, strictly validated as `vault:vN:<base64 64 bytes>` | references missing → `missing_references`; Vault unreachable → `unreachable`; reachable-but-key-wrong → `degraded` | UNIT_TEST (wire construction, signature-envelope parsing, fail-closed paths, key-reference normalization). NO live Vault round-trip performed |
| **kms** | **REAL SigV4-signed AWS KMS integration** (`crates/server/src/custody/kms/`): `TrentService.GetPublicKey` + `TrentService.Sign` with `EDDSA_SHA_512` (AWS KMS supports Ed25519 since Nov 2025). Hand-rolled SigV4 using workspace hmac/sha2 — no AWS SDK dependency; credentials read from the standard AWS env chain at request time, never rendered | Real: signature comes only from a KMS `Sign` response, strictly validated to 64 bytes | missing credentials/key → `configured_unsupported` with the exact dependency; unreachable → `unavailable` | UNIT_TEST (SigV4 signing key verified against the AWS-documented test vector; deterministic signed-request construction; SPKI parsing; fail-closed paths). NO live KMS round-trip performed |
| hsm | Explicit fail-closed refusal (`HsmCustodyProvider`) | Never | `configured_unsupported` — exact dependency named: PKCS#11 module with HSM_SLOT + HSM_PIN reference | CODE (refusal) + unit tests |

## Hard rules enforced in code

* No fake signatures: every signing path produces a signature from a
  real provider response or fails closed.
* The service token / AWS credentials never appear in errors, `Debug`,
  logs, or audit rows (redaction wrappers + transport-error scrubbing,
  unit-tested).
* Signer resolution verifies: provider type → signer status → key
  reference → key type (ed25519 / ECC_ED25519) → **public-key match**
  (a stale or wrong pinned key refuses with `PubkeyMismatch`, never
  signs).
* Tenant binding: tenant A cannot use tenant B's signer
  (integration-tested).
* Rotation (the P0 fix, 2026-09-30): `POST /api/saas/custody/rotations`
  resolves the REAL custody profile from the authenticated organization
  and requires `profile_id`; synthetic profile ids are impossible, both
  signers must exist, be active, and belong to the profile.

## What must happen before claiming production custody

A LIVE_TEST against a real Vault (transit ed25519 key) or real AWS KMS
(ECC_ED25519 key): health probe, resolve, sign, verify. Until then the
honest statement is: *implemented and unit-tested, not live-proven.*
