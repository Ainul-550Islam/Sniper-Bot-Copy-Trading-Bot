# IP Ownership Register — sniper-suite 0.1.0

> Do not invent ownership. `LEGAL_REVIEW_REQUIRED` where evidence insufficient. All paths exist.

| Component | Repository Path | Origin | License | Ownership Evidence | Handover Requirement |
|---|---|---|---|---|---|
| **Workspace root** | `Cargo.toml`, `VERSION`, `CHANGELOG.md` | Internal | Proprietary | `LICENSE` (all rights reserved), `Cargo.toml` `license.workspace = "MIT"` | Repo transfer |
| **core** | `crates/core/src/{lib.rs,config,state,models,billing,custody,db,ownership,obs}` | Internal | Proprietary | `crates/core/Cargo.toml` `LicenseRef-Proprietary`, `CHANGELOG.md` Batch1-5, `release-manifest.json` | Repo transfer |
| **solana-kit** | `crates/solana-kit/src/{lib.rs,rpc,signer,tokens}` | Internal (wraps `solana-sdk/client` 2.1 Apache-2.0) | Proprietary (kit) + Apache-2.0 (solana) | `crates/solana-kit/Cargo.toml` MIT, `licenses.json` `solana-sdk 2.1 Apache-2.0` | Repo transfer |
| **module-sniper** | `crates/module-sniper/src/` | Internal | Proprietary | `Cargo.toml` `LicenseRef-Proprietary`, `docs/SNIPER-ENGINE.md` | Repo transfer |
| **module-copy** | `crates/module-copy/src/` | Internal | Proprietary | `Cargo.toml` `LicenseRef-Proprietary`, `docs/COPY-TRADING-*.md` | Repo transfer |
| **module-polymarket** | `crates/module-polymarket/src/` | Internal | Proprietary | `Cargo.toml` `LicenseRef-Proprietary`, `docs/POLYMARKET-*.md` | Repo transfer |
| **module-telegram** | `crates/module-telegram/src/` | Internal | Proprietary | `Cargo.toml` `LicenseRef-Proprietary`, `docs/MODULES.md` | Repo transfer |
| **server** | `crates/server/src/{main.rs,api.rs,ops/*,backup/*,saas/*,security/*}` | Internal | Proprietary | `crates/server/Cargo.toml` `LicenseRef-Proprietary`, ops + backup modules | Repo transfer |
| **saas-sdk** | `crates/saas-sdk/src/` | Internal | Proprietary | `LICENSE` (all rights reserved; Cargo.toml is `LicenseRef-Proprietary`), `cargo test -p saas-sdk` harness exists | Repo transfer |
| **staking-suite program** | `programs/staking-suite/src/lib.rs` | Internal (distinct, own Cargo.lock) | Proprietary (`programs/staking-suite/Cargo.toml` is `LicenseRef-Proprietary`) | `Cargo.lock` solana 2.1, `cargo build-sbf` .so 187KB (not committed, sha `57a890fa…`), `program_id` placeholder `3vEEMM...` | Repo transfer + buyer `staking-identity.sh set-id` |
| **frontend** | `apps/control-plane/{src,package.json,package-lock.json}` | Internal (Next.js 16 MIT, React 19 MIT) | Proprietary (code) | `LICENSE` (all rights reserved; framework deps stay on their own licenses), `package-lock.json` 6171 lines v3 (Batch 11) | Repo transfer |
| **migrations** | `crates/core/migrations/*.sql` (high water `<!-- stat:migrations_high_water -->0054<!-- /stat -->`) | Internal | Proprietary | Forward-only, <!-- stat:migrations -->54<!-- /stat --> contiguous, `verify-delivery.sh` PASS | Repo transfer |
| **docs** | `docs/*.md` (<!-- stat:docs_canonical -->101<!-- /stat -->) | Internal | Proprietary (docs) | `docs/DATA-ROOM-INDEX.md` | Repo transfer |
| **third-party Rust** | `Cargo.lock` 707 entries (tokio, axum, sqlx, redis, spl, etc.) | External | MIT/Apache-2.0/BSD (see `licenses.json`) | `licenses.json`/`sbom.json` generated | Keep notices, `cargo deny` |
| **third-party JS** | `apps/control-plane/package-lock.json` next/react | External | MIT | `package.json` | Keep notices |
| **generated** | `target/`, `node_modules/`, `sbom.json` (generated) | Generated | — | .gitignore'd, excluded from `buyer-release` | Regenerate, not transfer |
| **staking program keypair** | `*-keypair.json` (not committed) | — | — | `.gitignore` `*-keypair.json`, `scripts/staking-identity.sh` refuses placeholder deploy | Buyer generates, `LEGAL_REVIEW_REQUIRED` if prior key exists |
| **trademark/domain** | — | — | — | `docs/TRADEMARK-DOMAIN-REGISTER.md` says NOT INCLUDED | LEGAL_REVIEW_REQUIRED (no evidence) |

## Unresolved Legal Review

| Item | Reason | Status |
|---|---|---|
| Copyright holder in `LICENSE` (“sniper-suite authors” generic) | Need real legal entity | LEGAL_REVIEW_REQUIRED — insert real holder before commercial use |
| `Cargo.toml` `repository` URL placeholder removed (no URL) | Need real remote URL | LEGAL_REVIEW_REQUIRED |
| Third-party `UNKNOWN` licenses in `licenses.json` (some crates lack license field) | Must treat as unknown | LEGAL_REVIEW_REQUIRED |
| Program `program_id` placeholder `3vEEMMFmdA...` (no keypair for it) | Buyer must `set-id` with own keypair | Buyer action, not legal |
| Any vendored/copied snippet without header | Checked `grep -R "Copyright"` — no hidden headers found, but buyer should re-scan | LEGAL_REVIEW_REQUIRED (generic) |

> **No claim** that all code is legally owned by seller — buyer to obtain counsel. Handover is repo transfer, not IP assignment until reviewed.
