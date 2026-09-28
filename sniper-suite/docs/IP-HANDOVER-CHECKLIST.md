# IP Handover Checklist — sniper-suite 0.1.0

> Distinguishes **seller-provided** vs **buyer-created**. Buyer must provision buyer-created items.

| Item | Description | Seller Provided? | Buyer Created? | Evidence / Path | Handover Action |
|---|---|---|---|---|---|
| **Source code** | 8 workspace crates + staking program | ✅ Seller | — | `Cargo.toml` 8 members, `crates/*`, `programs/staking-suite` | Transfer GitHub repo (see `docs/IP-OWNERSHIP-REGISTER.md`) |
| **Documentation** | 70 → 101 `docs/*.md` + `README.md`, `AUDIT.md` | ✅ | — | `docs/DATA-ROOM-INDEX.md` | Repo transfer |
| **Trademarks / names** | Project name “sniper-suite” | ❌ NOT INCLUDED | Buyer to register if desired | `docs/TRADEMARK-DOMAIN-REGISTER.md` says NOT INCLUDED | LEGAL_REVIEW_REQUIRED |
| **Domains** | No domain included | ❌ NOT INCLUDED | Buyer to acquire | `TRADEMARK-DOMAIN-REGISTER` | — |
| **Package registries** | Not published to crates.io / npm | ❌ NOT INCLUDED | Buyer to publish if desired | `Cargo.toml` `publish = false` (implied) | Buyer `cargo publish` |
| **GitHub repository** | Current remote (note: placeholder `repository` removed from `Cargo.toml`) | ✅ (private repo) | — | `git remote -v`, `docs/SOURCE-OF-TRUTH.md` | Transfer ownership / add collaborator |
| **CI configuration** | `.github/workflows/{ci.yml,frontend-ci.yml}` (fmt/check/clippy/test/security/docker/release/external-gated) | ✅ | — | `.github/workflows/ci.yml` | Buyer enable Actions, add secrets `POSTGRES_URL`/`REDIS_URL` if needed |
| **Deployment scripts** | `Dockerfile` (`rust:1.98.1-bookworm`), `docker-compose.yml`, `.env.template`, `config.toml.example` | ✅ | Buyer secrets | `Dockerfile`, `scripts/build-release-package.sh` | Buyer builds `docker build -t sniper-suite:prod .` |
| **Signing / custody config** | `core/custody/provider_config.rs` + `credentials.rs` indirect refs, `solana-kit/signer` | ✅ (refs only) | Buyer Vault/KMS/HSM cluster | `docs/SECRETS-MANAGEMENT-MATRIX.md` | Buyer provisions Vault, sets env `VAULT_ADDR` etc. |
| **Cloud resources** | No cloud account included | ❌ | Buyer to provision | — | Buyer provisions Postgres 16, Redis 7, RPC, OTLP |
| **Customer data, if any** | No customer DB included (migrations empty) | ❌ NOT INCLUDED (no dump) | Buyer `pg_dump`/`pg_restore` via `backup/*` | `docs/BACKUP-RESTORE.md` + `backup/commands.rs` | Buyer creates tenants via `POST /api/saas/organizations` |
| **Third-party licenses** | MIT/Apache/BSD/UNKNOWN via `licenses.json` 707 entries | ✅ (inventory) | — | `licenses.json`, `sbom.json`, `docs/THIRD-PARTY-SOFTWARE-INVENTORY.md` | Preserve `LICENSE` + per-dep notices |
| **SBOM / compliance** | `sbom.json`, `licenses.json` generated 2026-09-24 | ✅ | Buyer regenerates after changes | `scripts/generate-sbom.sh` | `bash scripts/generate-sbom.sh` |
| **Release artifacts** | `buyer-release/` (source/docs/manifests/sbom/licenses/checksums) | ✅ | — | `buyer-release/checksums/SHA256SUMS` | Verify sha, `bash scripts/verify-buyer-package.sh` |
| **Staking program so** | `programs/staking-suite/target/deploy/staking_suite.so` (187KB, sha `57a890fa…` — **not committed**) | ⚠️ Built artifact, placeholder `program_id` `3vEEMM...` | Buyer generates keypair `scripts/staking-identity.sh set-id` | `docs/STAKING.md`, `scripts/staking-identity.sh` | Buyer `solana-keygen new` → `set-id` → `deploy` |
| **Customer/revenue** | No traction claimed | ❌ NOT INCLUDED | — | `docs/KNOWN-LIMITATIONS.md` | — |

> **Legal note:** See `docs/IP-OWNERSHIP-REGISTER.md` for per-component origin/internal/external and `LEGAL_REVIEW_REQUIRED` flags.
