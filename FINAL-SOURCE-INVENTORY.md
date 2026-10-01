# FINAL-SOURCE-INVENTORY

Machine-generated from the staged release tree. No number in this file is
hand-written; every count is computed by walking the tree at generation time.

- Repository root (source of truth): /home/user/Sniper-Bot-Copy-Trading-Bot
- Staged release root: FINAL-COMPLETE-SOURCE/
- Generation timestamp (UTC): 2026-10-01T13:34:20Z
- Total product files: 859
- Total directories: 109
- Total source size (bytes): 11,132,593 (10.6 MiB)

## By file class
- Rust files (*.rs): 570
- TypeScript files (*.ts): 7
- TSX files (*.tsx): 22
- JavaScript files (*.js/*.mjs/*.cjs): 1
- SQL files (*.sql): 35
  - Migrations (crates/core/migrations/*.sql): 35 (high-water 0035)
- Markdown files (*.md): 137
- Shell scripts (*.sh): 25
- CSS/SCSS files: 1
- HTML files: 0
- JSON files: 32
- TOML files: 15
- YAML files: 3
- Config files (toml/yaml/Dockerfile/gitignore/env+config templates/tsconfig/package*/next/eslint/compose): 28
- Test files (reliably detectable: files under a tests/ directory, or *.test.* / *.spec.* frontend files): 101
  - of which Rust integration test files under a tests/ directory: 101
  - of which frontend test/spec files: 0

## Included top-level entries
- .cargo
- .dockerignore
- .env.template
- .github
- .gitignore
- AUDIT-OPEN-ITEMS-2026-10-01.md
- AUDIT-REMEDIATION-2026-09-29.md
- AUDIT.md
- CHANGELOG.md
- Cargo.lock
- Cargo.toml
- DONE.md
- Dockerfile
- LICENSE
- PROMPT-2-RESULT.md
- PROMPT-3-RESULT.md
- PROMPT-4-PHASE0-MATRIX.md
- PROMPT-4-POLYMARKET-RESEARCH.md
- PROMPT-4-PROGRESS.md
- PROMPT-4-RESULT.md
- PROMPT-5-RESULT.md
- PROMPT-5-SPEC.md
- README.md
- SECURITY.md
- VERSION
- apps
- config.toml.example
- crates
- deny.toml
- docker-compose.yml
- docs
- evidence
- licenses.csv
- licenses.json
- programs
- release-manifest.json
- rust-toolchain.toml
- rustup-init.sh
- sbom.cyclonedx.json
- sbom.json
- scripts
- tests

## Workspace members (crates)
- crates/core
- crates/solana-kit
- crates/module-sniper
- crates/module-copy
- crates/module-polymarket
- crates/module-telegram
- crates/server
- crates/saas-sdk
- crates/core
- crates/solana-kit
- crates/module-sniper
- crates/module-copy
- crates/module-polymarket
- crates/module-telegram

## Standalone program workspace
- programs/staking-suite (own Cargo.lock; root rust-toolchain.toml pin does not apply inside it)

## Excluded from this source release (with reasons)
- buyer-release/ — generated buyer handover package, gitignored; regenerable deterministically via scripts/rebuild-buyer-release.sh (remains in the working tree)
- target/, node_modules/, .next/, out/, dist/, build/, coverage/, .turbo/, .vercel/ — build artifacts / caches (all gitignored)
- .cargo/bin/, .cargo/env — machine-local rustup toolchain state accidentally captured under the repo's .cargo/ (cargo home for this build is ~/.cargo, outside the repo); .cargo/audit.toml IS included (project config)
- .config/solana/install/config.yml — machine-local Solana/Agave installer state (points at /home/user/.local paths; no project value)
- apps/control-plane/tsconfig.tsbuildinfo — TypeScript incremental-build artifact (gitignored *.tsbuildinfo)
- .git/ — absent in the working tree (no Git history to include; .gitignore itself IS included)

## Excluded secret classes (none found staged; scan commands in FINAL-SOURCE-VALIDATION.md)
- real .env files / private keys / wallet keypairs / seed phrases / PEM keys
- provider credentials (Stripe, Paddle, Vault, AWS KMS, Telegram bot tokens)
- database or production credentials

## Self-exclusion rule
FINAL-SOURCE-MANIFEST.txt and FINAL-SOURCE-SHA256.txt cover the 859 PROJECT files
above. The five packaging artifacts (FINAL-SOURCE-INVENTORY.md, FINAL-SOURCE-MANIFEST.txt,
FINAL-SOURCE-SHA256.txt, FINAL-SOURCE-VALIDATION.md, FINAL-COMPLETE-SOURCE-REPORT.md)
are generated last and are intentionally NOT self-referenced in the manifest or
hash file (a file cannot contain its own hash); their presence is verified in the
ZIP extraction comparison instead.
