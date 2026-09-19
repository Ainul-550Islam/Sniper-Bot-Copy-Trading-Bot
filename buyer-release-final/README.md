# sniper-suite — FINAL buyer-handover package

Read this first. Everything in this package is verifiable with the commands
shown; nothing here asks you to trust a claim you cannot check.

## 1. What you received

A complete trading-system source tree (Rust workspace: control plane +
sniper/copy-trade/Telegram/Polymarket modules + Solana kit), a Solana
staking program (source + deterministically built `.so`), 47 documentation
files, execution evidence (test/gate/build/DB/security logs), provenance
hashes, and deployment material. Exact inventory: `provenance/SOURCE-PROVENANCE.json`.

## 2. What the product is (factual)

Modular crypto trading suite v0.1.0, MIT-licensed: paper trading by
default; live trading only behind an explicit owner-key + config two-key
ceremony; Polymarket (Polygon) integration with hand-written EIP-712 v2
order signing; Solana sniping/copy-trading with simulate-first execution;
PostgreSQL durable state + Redis coordination; REST API (21 routes) +
Telegram control (6 commands); native Solana staking program with hard
parameter caps. No customers, revenue, or track record is claimed — none
exists.

## 3. Verify integrity first

```
cd buyer-release-final && sha256sum -c checksums/SHA256SUMS
```
Expected: every line `OK`, 0 failures. Cross-check the source tree hash
against `provenance/SOURCE-PROVENANCE.json` (`tree_sha256` =
`033b582f79c5e1d24a301895fa98d37fae1ed01de15f0052a4b2777a6dd06946`,
183 files / 3,293,320 bytes / 86,576 lines). Reproduction steps for every
hash: `provenance/SOURCE-PROVENANCE.md`.

## 4. Reading order

1. `docs/BUYER-OVERVIEW.md` — what the system does and does not do.
2. `docs/BUYER-REPRODUCTION-GUIDE.md` — clean-machine build/test procedure.
3. `docs/BUYER-ACCEPTANCE-TEST.md` — the A–R acceptance record you fill in.
4. `docs/FINAL-KNOWN-LIMITATIONS.md` — honest 6-category limits register.
5. `docs/FINAL-RELEASE-AUDIT.md` — the vendor's closing audit of this pass.

## 5. Reproduce the build

Follow `docs/BUYER-REPRODUCTION-GUIDE.md` (toolchain pins: rust 1.98.1,
agave 2.1.21, platform-tools v1.43). Recorded vendor results on Rust
sources byte-identical to this tree: 537 + 537 (all-features) workspace
tests, 71 staking host tests, 3 validator-e2e tests, 23 DB-suite rerun —
**0 failures**; `release-check.sh` 20/20 (executed on the hardening tree —
Rust-level gates byte-identical to this tree, doc-level gates re-covered
here); `verify-delivery.sh` 7/7 re-executed on this final tree. Your runs
are the acceptance evidence; vendor logs are references.

## 6. Run it (paper mode)

`docs/BUYER-QUICKSTART.md` + `docs/BUYER-DEPLOYMENT.md`: PostgreSQL +
Redis + `CONFIG_PATH=./config.toml ./target/debug/sniper-suite`. Paper
mode is the default and the only mode this package's evidence covers.

## 7. Deployment options

Native binary (vendor-executed, evidence included), Docker/compose
(material included; **vendor-side docker run = BLOCKED**, no daemon in the
build environment — your acceptance test G1), CI workflow included
(**vendor-side GitHub run = BLOCKED**; 1:1 local equivalents PASS, see
`ci/CI-LOCAL-EQUIVALENCE.md`).

## 8. Staking program status — read carefully

The `.so` (`binaries/staking_suite.so`, sha256 `57a890fa…5564`) is
deterministic (byte-identical cold rebuild proven) and passed 71 host + 3
on-VM e2e tests against a mainnet-cloned metadata program. **It is NOT
deployed. The declared program id is a documented PRE-DEPLOYMENT
PLACEHOLDER.** Deployment is a HUMAN ACTION: generate your keypair,
`scripts/staking-identity.sh set-id`, rebuild, deploy via the same script
(it refuses mismatched keypairs and placeholder ids on public clusters).
Details: `staking-program/PROGRAM-ID-STATUS.md`, `docs/STAKING.md`.

## 9. Going live (trading)

Live trading requires: explicit config enablement + owner-role API key
(two-key ceremony), funded wallets, your RPC endpoints, and completion of
the acceptance sections N–Q. Live-mode safety gates (vault/kms/hsm signer
backends fail startup; kill-switch; risk engine before execution) are
described in `docs/SECURITY-BOUNDARY-MAP.md` and exercised in paper mode
only by the vendor. No live/mainnet execution evidence exists or is
claimed.

## 10. Operations

Day-2 procedures (start/stop, backups, rotation of every credential,
rollback, reconciliation, recovery): `docs/FINAL-OPERATIONS-HANDOVER.md`.
Incident handling, 17 scenarios: `docs/FINAL-INCIDENT-RUNBOOK.md`.

## 11. Security posture — what is and is not true

True: 0 `unsafe` blocks, `forbid(unsafe_code)` in 4 crates, dependency
audit/deny clean at recorded run time, secret scans clean, digest-only key
storage, RBAC + audit trail, fail-closed money-path rules (verified by
inspection + executed tests). **Not true / not claimed: external security
audit (none exists), encryption-at-rest guarantees, DDoS resistance,
profit or latency guarantees.** Full boundary analysis with per-boundary
limits: `docs/SECURITY-BOUNDARY-MAP.md`; register: `docs/FINAL-KNOWN-LIMITATIONS.md`.

## 12. Known gaps (vendor environment)

BLOCKED vendor-side: Docker run, GitHub Actions run (materials + local
equivalents included). NOT RUN: funded live validation, landing-rate
benchmark, external audit. HUMAN ACTION: everything credential-bearing,
program deployment, live enablement, legal/IP review (factual inventory:
`docs/FINAL-IP-AND-THIRD-PARTY-INVENTORY.md`). These are environment and
ownership boundaries — they are recorded, never silently converted to PASS.

## 13. Acceptance and history

Sign-off happens through `docs/BUYER-ACCEPTANCE-TEST.md` (sections A–R;
each with command, expected result, and blank ACTUAL/PASS-FAIL/INITIALS/
DATE fields — nothing is pre-marked PASS). Machine-readable status:
`BUYER-FINAL-RELEASE-MANIFEST.json` (statuses restricted to PASS / NOT RUN
/ BLOCKED / HUMAN ACTION). An independent 23-rule zero-trust adversarial
audit re-verified this package on 2026-09-19 (found and fixed 5
documentation/integrity defects, 0 Rust defects; logs:
`evidence/handover/adversarial-audit/`, summary: `docs/FINAL-RELEASE-AUDIT.md` §H). The previous hardening-pass package
(`buyer-release/`) is preserved untouched as labeled history; its manifest
copy here is filename-suffixed `-HISTORICAL`. Superseded evidence is kept
and labeled, never deleted.
