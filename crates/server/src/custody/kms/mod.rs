//! AWS KMS custody integration (§H, spec files 53–57).
//!
//! A REAL AWS KMS integration built on the workspace's `reqwest` +
//! `hmac` + `sha2` dependencies: SigV4 request signing is implemented in
//! [`client`] (verified in tests against the test vector published in
//! the AWS Signature Version 4 documentation), and AWS KMS has supported
//! Ed25519 (`ECC_ED25519` / `EDDSA_SHA_512`) signing since November
//! 2025 — exactly what this Solana-signing custody boundary needs. No
//! AWS SDK dependency is required for the environment-credential-chain
//! deployment shape.
//!
//! The module layout:
//!
//! * [`config`] — reference-only configuration (`KMS_KEY_ID`,
//!   `KMS_REGION`, optional `KMS_ENDPOINT`); AWS credentials are read
//!   from the standard environment chain at request time, never stored
//!   on long-lived structures, and never rendered;
//! * [`client`] — the SigV4-signed HTTP client wrapper:
//!   `TrentService.GetPublicKey` and `TrentService.Sign`
//!   (`EDDSA_SHA_512`), plus the strict Ed25519 SPKI parser;
//! * [`signer`] — `KmsSigner` (implements the core `CustodySigner` trait;
//!   every signature comes from a real KMS `Sign` response) and the
//!   server-side `KmsCustodyProvider` adapter with full fail-closed
//!   resolution checks (provider type, status, key reference,
//!   credentials, key spec, public-key match);
//! * [`health`] — the ordered references → authorization → key-type
//!   probe and its `ProviderHealth` mapping.
//!
//! What this integration will never do: fabricate a signature, fall back
//! to a local key, expose AWS credentials, or report healthy without an
//! authenticated round-trip to KMS. When credentials or the key are
//! missing, every path fails closed with the exact missing dependency
//! ([`client::AWS_KMS_DEPENDENCY`]).
//!
//! Verification honesty: the SigV4 derivation is unit-tested against
//! the AWS-documented test vector and the wire construction is unit
//! tested, but NO live AWS round-trip has been performed in this
//! workspace — a deployment must run its own LIVE_TEST with real
//! credentials before enabling `CUSTODY_PROVIDER=kms`.

pub mod client;
pub mod config;
pub mod health;
pub mod signer;

pub use client::{
    parse_ed25519_spki, sigv4_signing_key, AwsEnvCredentials, KmsClient, KmsClientError,
    KmsPublicKey, KmsSignedRequest, AWS_KMS_DEPENDENCY, EDDSA_SHA_512,
};
pub use config::{KmsConfig, KmsReferenceState};
pub use health::{check_kms_readiness, deployment_readiness, KmsReadiness};
pub use signer::{kms_dependency_text, KmsCustodyProvider, KmsSigner};
