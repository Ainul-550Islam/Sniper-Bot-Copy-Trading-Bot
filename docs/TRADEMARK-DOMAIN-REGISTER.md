# Trademark / Domain / Brand Register — sniper-suite 0.1.0

> Inventory per instruction: if none included, explicitly state NOT INCLUDED / NOT VERIFIED. Never invent registrations.

## Project / Product / Brand Assets

| Asset | Status | Evidence | Notes |
|---|---|---|---|
| Project name “sniper-suite” / “Sniper Suite” | **NOT VERIFIED** | No trademark certificate in repo; `grep -R -i "trademark" docs` none | Generic; buyer to search USPTO/EUIPO |
| Logo / brand assets | **NOT INCLUDED** | No `assets/logo.*` in repo | — |
| Domain `sniper-suite.com` (example) | **NOT INCLUDED** | No domain file, no registrar record | Buyer to acquire if desired |
| Social handles | **NOT INCLUDED** | — | — |
| App store listings | **NOT INCLUDED** | — | — |
| Solana program name “staking-suite” | **NOT VERIFIED** | `programs/staking-suite/Cargo.toml` name, placeholder `program_id` `3vEEMM...` | Buyer to `staking-identity.sh set-id` |

## Package Registries

| Registry | Package | Status | Evidence |
|---|---|---|---|
| crates.io | `sniper-suite`, `bot-core`, `saas-sdk` | **NOT PUBLISHED** | `Cargo.toml` no `publish` restriction but `cargo publish` never run; `release-manifest.json` no registry URL |
| npm | `control-plane` | **NOT PUBLISHED** | `apps/control-plane/package.json` `private` (assumed), no `npm publish` |
| Docker Hub | `sniper-suite:prod` | **NOT PUBLISHED** | `Dockerfile` exists, `ci.yml` `docker` job `push: false` |
| Solana program | `staking_suite.so` on mainnet | **NOT DEPLOYED** | Placeholder `program_id` `3vEEMMFmdA...`, `scripts/staking-identity.sh` refuses placeholder deploy |

## Repository & CI

| Asset | Status | Evidence |
|---|---|---|
| GitHub repository | **PROVIDED** (private, current tree) | `git remote -v` (buyer to transfer), `docs/SOURCE-OF-TRUTH.md` |
| CI | **PROVIDED** `.github/workflows/{ci.yml,frontend-ci.yml}` | `ci.yml` 4 jobs + `release` + `external-gated` |

## Verdict

**All brand/trademark/domain/registry assets are NOT INCLUDED / NOT VERIFIED** unless buyer verifies otherwise. This transaction is **source code + documentation + migrations + scripts** (MIT), not a brand sale.

- No domain is transferred.
- No trademark is assigned.
- No app store entry is included.
- Program must be deployed under buyer’s own keypair.

> **Buyer action:** If brand is needed, commission trademark search and domain acquisition; for program, run `solana-keygen new` + `bash scripts/staking-identity.sh set-id <keypair>` before `deploy`.

*Verification:* `grep -R -i "trademark\|domain" docs` (only this file), `cat Cargo.toml | grep repository` (no URL), `cat docs/IP-HANDOVER-CHECKLIST.md` (marks trademarks NOT INCLUDED).
