# Final release audit (buyer-handover pass, 2026-09-19)

This document is the closing audit record of the FINAL BUYER-HANDOVER PASS
(22 phases) executed on top of the buyer-hardened tree. Rule compliance: no
architecture rebuilt, no working code replaced, no functionality removed, no
evidence/customers/revenue/certification/mainnet claims invented; every
number recalculated from the final tree; superseded evidence preserved and
labeled. All artifacts live in the release package (`buyer-release-final/`)
and the vendor evidence tree; per-claim mapping in `docs/EVIDENCE-INDEX.md`.

## A. Pass actions by phase

| Phase | Action | Result |
|---|---|---|
| 1 Source provenance | Full re-scan: per-file path/bytes/sha256/exec-bit + tree hash + archive/manifest/binary/lockfile hashes + toolchain + build-env + OS | `provenance/SOURCE-PROVENANCE.{json,md}` (package + evidence) |
| 2 File completeness | Four-way check: live repo vs source-tree mirror vs tarball vs package | repo = mirror = tarball; 0 missing / 0 extra / 0 mismatched (re-run at handover; earlier hardening-pass three-way check also 175/175) |
| 3 Placeholder audit | 10-pattern scan of all text files + manual classification of every hit | 0 real defects; classification table in §C |
| 4 Feature traceability | Symbol-verified matrix (file exists + symbol grep + test target exists) | `docs/FEATURE-TRACEABILITY.md` (12 areas, every row machine-verified) |
| 5 Security boundary trace | 13-boundary map, 8 questions each, code-referenced | `docs/SECURITY-BOUNDARY-MAP.md` |
| 6 Money-path audit | Inspection of Polymarket/Solana/Staking money paths + fail-closed verification against executed tests | No new defects; no code changes needed (findings in §D) |
| 7 Deployment identity | `staking-identity.sh verify` re-run + TRACKED_FILES audit | **1 real release defect found + fixed** (§B); verify re-run PASS, 4 tracked docs |
| 8 Secret hygiene | Full-tree + evidence + package scans | 0 secrets (results in §E) |
| 9 IP/third-party inventory | LICENSE, both lockfiles, cargo trees, deny policy, hand-written integrations | `docs/FINAL-IP-AND-THIRD-PARTY-INVENTORY.md` |
| 10 Buyer reproduction | Clean-machine procedure with exact commands + expected outputs | `docs/BUYER-REPRODUCTION-GUIDE.md` |
| 11 Acceptance test | Upgraded to formal A–R record; nothing pre-marked PASS | `docs/BUYER-ACCEPTANCE-TEST.md` |
| 12 Limitations register | 6 strict categories + explicit non-claims | `docs/FINAL-KNOWN-LIMITATIONS.md` |
| 13 Operations handover | Startup→rotation→rollback incl. 16 sections | `docs/FINAL-OPERATIONS-HANDOVER.md` |
| 14 Incident runbook | 17 scenarios × 6 fields + universal checklist | `docs/FINAL-INCIDENT-RUNBOOK.md` |
| 15–17 Package/hashes/archive | `buyer-release-final/` built; SHA256SUMS{,.json}; reproducible archive + double-extraction compare | package self-check 0 failures; archive byte-compared against mirror |
| 18 Final manifest | `BUYER-FINAL-RELEASE-MANIFEST.json` — statuses restricted to PASS / NOT RUN / BLOCKED / HUMAN ACTION | in package root |
| 19 Buyer README | First-read document, factual | `buyer-release-final/README.md` |
| 20 Commercial boundary | Claim scan across all buyer docs | 1 correction (README multisig "audited" wording); all other hits classified legitimate (audit-trail sense / scoped technical guarantees / explicit negations) |
| 21 Code-change rule | Only real defects modified; complete final file content produced | 1 script modified (`scripts/staking-identity.sh`, full content output); 0 Rust changes |
| 22 This audit | 23-point summary below | this file |

## B. Defect found and fixed this pass (the only source change)

**`scripts/staking-identity.sh` (release/deployment defect):**
1. `TRACKED_FILES` did not include `docs/BUYER-ACCEPTANCE-TEST.md`, which
   quotes the declared program id — after a buyer `set-id`, that document
   would have retained the stale placeholder (violating "no stale
   placeholders in buyer docs"). FIX: added to `TRACKED_FILES`; `verify`
   re-run passes with 4 tracked docs (`evidence/phase7-identity-verify.log`).
2. The post-`set-id` stale-reference sweep scans `*.json`, which would have
   flagged `release-manifest.json` — a delivery-time record that must keep
   the as-delivered placeholder (like AUDIT.md). Left unfixed, `verify`
   would false-FAIL for every buyer after `set-id`. FIX: sweep excludes
   `release-manifest.json` with a documented rationale + operator NOTE.
Both fixes were verified by execution (`bash -n`, `verify` exit 0, and a
simulated post-set-id sweep returning exactly the tracked-file set). The
complete final file content was produced (no diff-only edit).

No Rust source was modified in this pass. Therefore all recorded test
evidence (537/537, 71/71, 3/3 e2e, release-check 20/0/0) remains valid for
the final tree's Rust sources: the last full-gate execution ran on Rust
sources byte-identical to the final tree (changes since are markdown/JSON
docs + this one shell script; script changes cannot affect compiled test
results). Gate applicability on the final 183-file tree, stated precisely:
`release-check.sh` 20/20 was executed on the 175-file hardening tree and
was NOT re-run here (no Rust toolchain in this pass) — its Rust-level gates
remain applicable via byte-identity, its doc-level gates were re-covered by
the placeholder scan (0 defects), doccheck (CLEAN) and identity verify
(PASS); `verify-delivery.sh` WAS re-executed on the final tree during the
independent adversarial audit (7/7 PASS, see §H).

## C. Placeholder-audit classification table (every hit class)

Scanner: 10 regex families over all 175+ text files (raw hits: 237; machine
output in package `evidence/handover/placeholder-scan-raw.json`). Manual
review result — exact classes:

| Class | Count | Representative examples (file:line) | Disposition |
|---|---|---|---|
| REAL DEFECT | **0** | — | nothing to fix in code |
| LEGITIMATE DOCUMENTATION | majority of doc hits | `docs/*` gate descriptions ("TODO/stub marker scan"), README localhost commands, `<YOUR_RPC>`-style operator placeholders in procedures | kept (required placeholders for buyer-supplied values) |
| LEGITIMATE TEST NARRATION | prose continuation comments | `crates/core/src/risk.rs:916` ("// ... and once the stop loss level is breached..."), `crates/solana-kit/src/pump.rs:1317` ("// ... but not by more than fee+slippage+rounding."), `mock_clob_gamma.rs:227` ("obviously-fake private scalar (never used on a real chain)") | kept |
| TEST FIXTURES (mock endpoints) | all `127.0.0.1:{0,1}` / `example.com` in `#[cfg(test)]`/tests | `collateral.rs:245/277/398`, `rpc.rs:1026-1042`, `main.rs:1237` (inside `#[cfg(test)]` at :1144), `obs.rs:762` (inside test mod at :596) | kept — verified each sits in test scope |
| REQUIRED PLACEHOLDER (tracked) | program id | `declare_id!("3vEEMM…9mfy")` + tracked doc quotes | tracked by `staking-identity.sh` (§B fix); deployment = HUMAN ACTION |
| DESIGN (paper-mode demo seeds) | 7 code hits | `server/main.rs:86` ("Seed demo balances in paper mode"), `module-sniper/lib.rs:326` (paper-only fallback rule) | kept — fail-closed in live (executed tests; §D) |
| TOOLING SELF-REFERENCE | 5 | `release-check.sh:96-102` (the marker gate's own grep pattern), `release-manifest.json:73` (gate description) | kept |
| UNSUPPORTED FEATURE markers | 0 | `unreachable!` ×1 is a TEST assertion arm (`validator_e2e.rs:279`); no `todo!`/`unimplemented!` anywhere; 0 `unsafe` | nothing to fix |
| HISTORICAL NARRATIVE | AUDIT.md hits incl. one "existing code" phrase in a 2026-09-17 phase heading (`AUDIT.md:275`, meaning "based on the existing codebase") and `roles(TODO)` in an early architecture sketch (`AUDIT.md:282`; roles were subsequently implemented — module RBAC tests executed) | append-only history preserved per policy | kept, labeled |

## D. Money-path audit findings (Phase 6)

* **Polymarket:** live sizing reads on-chain collateral only
  (`collateral.rs`: `balanceOf`/`decimals`/`allowance` via `eth_call`;
  `rpc_errors_are_errors_never_zero` executed); `Err` = reject, never zero,
  never paper fallback (`lib.rs` `BalanceUnavailable`/`InsufficientFunding`,
  "No fallback exists" + freshness bound on snapshots; decimals validated
  before conversion; `usd_to_raw` rejects non-finite/negative/overflowing).
* **Solana:** `available_sol` — cached/demo balance substitutes ONLY in
  explicit paper mode with a positive cached value (pure rule
  `sol_balance_fallback`, unit-tested); simulate/live propagate real RPC
  errors → risk rejection. `may_broadcast` hard gate requires
  `allow_live_trading` even in live mode (downgrades to Simulate
  otherwise). Simulate-first: `SimulationFailed` = never broadcast;
  broadcast failures classified terminal vs outcome-unknown (`SendUnknown`
  → reconciliation, never assumed failed).
* **Staking:** `overflow-checks = true` in the program profile + explicit
  `checked_*`/`saturating_*` arithmetic (9 call sites in `state.rs`, 4 in
  `processor.rs`); zero-cap/one-over-cap/exact-cap boundaries executed on
  the real BPF VM; zero-headroom claims safe; genesis latched; cap
  IMMUTABLE; metadata one-shot + replay rejected against the REAL mpl
  program; fee ≤10% / reward ≤100% APR / timelock ≤30d bounds enforced and
  executed.
* **Conclusion:** no gap requiring a new regression test was found — each
  rule above is already pinned by executed tests inside the recorded
  537/71/3 evidence runs. No code changed.

## E. Secret hygiene (Phase 8)

Scans over repo + evidence + package: 0 secret-pattern hits (release-check
regex families re-run), 0 keypair files in the repo tree, `.env.template`
contains variable NAMES only, evidence DB dumps contain test-seeded data
from throwaway local databases (no keys — the app never stores private
keys in PG; signer material is file-based and buyer-supplied), no tokens in
any log (redaction regression-tested). The release package excludes
`.env`, caches, histories, and target dirs by construction (hygiene gate
executed).

## F. Final counts (recalculated from the final tree — Phase 22 items 1–6)

| # | Item | Value |
|---|---|---|
| 1 | FINAL FILE COUNT | 183 |
| 2 | FINAL LINE COUNT | 86,576 |
| 3 | FINAL BYTE COUNT | 3,293,320 |
| 4 | FINAL TREE SHA-256 | see `provenance/SOURCE-PROVENANCE.json` (`tree_sha256`) — computed after this file reached its final size (self-referential hashes cannot be embedded in the hashed tree) |
| 5 | FINAL ARCHIVE SHA-256 | see `BUYER-FINAL-RELEASE-MANIFEST.json` (`archive_sha256`) + `provenance/SOURCE-PROVENANCE.json` |
| 6 | FINAL BINARY SHA-256 | `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` (staking_suite.so, 187,504 B; byte-identical across first build, incremental rebuild, and full cold rebuild) |
| 7 | TOTAL TESTS (recorded executions on identical Rust sources) | 537 workspace (+537 all-features) + 71 staking host + 3 validator e2e + 23 db-suite rerun on restored DB |
| 8 | FAILED TESTS | 0 |
| 9 | BLOCKED ITEMS | Docker build/compose/smoke (no daemon); GitHub Actions run (no runner) |
| 10 | HUMAN ACTION ITEMS | final program keypair/id + deployment (+upgrade/payer authorities, cluster); funded live validation; landing-rate bench; external security audit; CI run URL; legal/identity fill-ins; all credentials |
| 11 | SOURCE CHANGES (this pass) | 1 file: `scripts/staking-identity.sh` (defect fix §B; complete final content produced). 0 Rust changes |
| 12 | PACKAGE CHANGES | new `buyer-release-final/` (this pass); prior `buyer-release/` preserved untouched as labeled history |
| 13 | DOCUMENTATION CHANGES | +8 docs (47 total), 1 upgraded in place (BUYER-ACCEPTANCE-TEST), 1 claim correction (README §20), count-sync updates (REPOSITORY-MAP, CHANGELOG, release-manifest) |
| 14 | EVIDENCE FILES CREATED | provenance pair (JSON+MD), placeholder-scan raw+classified, identity re-verify log, handover-pass completeness CSV, final SUMS{,.json} — plus all preserved hardening evidence |
| 15 | SECURITY STATUS | statics/deps PASS (0 unsafe, forbid ×4, audit 0 errors ×2, deny ok, scans clean); external audit NONE EXISTS (claimed nowhere) |
| 16 | DEPLOYMENT STATUS | app deployable via Docker/compose/binary (Docker run = buyer-side, BLOCKED vendor-side); staking program UNDEPLOYED (placeholder id; guarded deploy path ready) |
| 17 | DATABASE STATUS | migrations 11/11; backup→restore round-trip EXECUTED incl. suite-on-restored-DB + app-on-restored-DB |
| 18 | DOCKER STATUS | BLOCKED (no vendor daemon); native-equivalent smoke PASS (labeled); buyer acceptance G1 |
| 19 | CI STATUS | BLOCKED (no vendor runner); 1:1 local equivalents PASS; buyer acceptance R2 |
| 20 | LIVE VALIDATION STATUS | NOT RUN (funded = HUMAN ACTION); procedure documented; simulate legs executed vs devnet |
| 21 | EXTERNAL AUDIT STATUS | NONE EXISTS — none claimed; recommended pre-mainnet |
| 22 | IP/THIRD-PARTY STATUS | inventory complete (docs/FINAL-IP-AND-THIRD-PARTY-INVENTORY.md); lockfile hashes recorded; no vendored/copied source; legal review = buyer counsel |
| 23 | FINAL ACCEPTANCE STATUS | buyer acceptance test A–R issued with vendor-evidence references; NO test pre-marked PASS; buyer execution required for sign-off |

## G. Environment of this pass (for reproducibility context)

Sandbox: 2-core x86_64 VM, ~2 GB RAM, Debian-based; PostgreSQL/Redis NOT
running during this documentation pass (no test re-execution was required —
§B rationale); Rust sources byte-identical to the fully-gated hardening
tree. Snapshot-mechanism caveats observed and handled: `target/`,
`~/.rustup`, `~/.cargo/registry` are environment-transient (wiped between
turns); exec bits restored and hash-verified; package dirs avoid
snapshot-excluded names (`build/` → `compile-logs/`). All persisted
evidence hashes re-verified intact at pass start (tree hash matched the
recorded `51726e7e…` pre-edit baseline).

## H. Independent adversarial audit (2026-09-19, post-packaging)

A zero-trust audit re-verified this pass's outputs treating every prior
claim, hash, and PASS as untrusted (23-rule protocol; full logs preserved
in the final package under `evidence/handover/adversarial-audit/`). Fresh
executions in the audit environment (no Rust toolchain available):
`verify-delivery.sh` on the final tree 7/7 PASS exit 0; independent
re-implementation of the tree-hash algorithm reproduced the documented
hash; archive rebuilt from a fresh extraction under the documented method
— byte-identical; SHA256SUMS recomputed by a second independent method —
312/312 OK; four-way repo/mirror/tarball/provenance comparison re-run —
0 diffs; per-file hash comparison against BOTH inventory generations —
183/183 and 168/175 unchanged + exactly the 15 documented changes;
negative-test mapping confirmed executed `ok` results for cap boundaries
(`genesis_rejects_one_over_the_cap`), RBAC denials, kill-switch, replay,
idempotency, `rpc_errors_are_errors_never_zero`,
`live_mode_rejects_failed_and_missing_reads`; secret/hidden-file scan of
the whole package — clean (only `.env.template`, dotfile configs, labeled
test-data dumps); no symlinks; program-id placeholder references all
labeled PRE-DEPLOYMENT; `set-id` fail-closed behavior observed (refuses
without `solana-keygen`). Defects found and fixed by the audit (all
documentation/integrity — zero Rust defects):

| # | Defect | Fix |
|---|---|---|
| D-A1 | Exec bits stripped by the environment snapshot mechanism on 3 repo scripts + 3 package-mirror scripts + 3 `deployment/scripts/` copies (content hashes unaffected; provenance/archive record `+x`) | `chmod +x` restored on all 9; verified repo = mirror = archive modes = provenance records |
| D-A2 | `EVIDENCE-INDEX.md` 71/71 row cited only `phase2-staking-clippy.log` (which contains no test-result line); pre-fix FAILED e2e logs (`phase5-validator-e2e.log` etc.) shipped unlabeled/unreferenced | citation completed (`release-check-final.log` §staking); FAILED logs explicitly labeled pre-fix history in the index |
| D-A3 | `release_check: 20/20 PASS` in the final manifest and package README lacked tree qualification (executed on the 175-file hardening tree; not re-runnable without toolchain) | manifest + README + this file now state applicability precisely |
| D-A4 | `FINAL-KNOWN-LIMITATIONS.md` VERIFIED bucket still cited the 175-file three-way baseline | updated to the final 183-file four-way verification (with the historical baseline labeled) |
| D-A5 | `verify-delivery-final.log` byte count (3,194,670) vs final hardening tree (3,196,164) unexplained — run predates that pass's last doc updates | historical-record note added to `EVIDENCE-INDEX.md`; log preserved unmodified; fresh 7/7 run on the final tree recorded |

No secrets, no content mismatches, no phantom checksum entries, no
unlabeled stale numbers remained after these fixes (re-scanned).
