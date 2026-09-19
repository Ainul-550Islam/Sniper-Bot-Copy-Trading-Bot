# Buyer-hardening pass — final report (2026-09-18)

18-phase re-verification of the audited sniper-suite tree. Rule compliance:
no manufactured evidence; every result traceable to command + artifact +
hash; strict PASS / NOT RUN / BLOCKED / HUMAN ACTION separation; working
logic preserved (2 defect fixes only, both proven by execution).

## 1. Final buyer-readiness matrix

| Area | Status | Evidence (package path) | Command | Artifact / hash | Reproducible? | Buyer action? | Blocker? |
|---|---|---|---|---|---|---|---|
| Source tree identity | PASS | evidence/inventory/source-inventory.{json,csv} | find+sha256 scanner | 175 files / 3,194,778 B / 85,152 lines; tree_sha256 `d9d07beb…4688bf` | yes | step 1 | none |
| Toolchain pins | PASS | evidence/gates/release-check-final.log (gate 3) | release-check | rustc/cargo 1.98.1, agave 2.1.21, platform-tools v1.43; no drift vs Dockerfile/CI | yes | step 2 | none |
| Metadata/check/fmt/clippy | PASS | evidence/compile-logs/phase2-{check,fmt}.log + gates log | cargo metadata --locked; check --workspace --all-targets; fmt --all --check; clippy --workspace --all-targets -D warnings | all exit 0 | yes | steps 2–3 | none |
| App tests (live services) | PASS | evidence/tests/phase2-test-workspace{,-allfeat}.log | cargo test --workspace [--all-features] -- --test-threads=1 | **537/537 ×2**, 0 failed; gated db/redis/distributed/two-replica EXECUTED (PG 17.11 + Redis 8.0.2) | yes | step 4 | none |
| Staking host tests | PASS | evidence/gates/release-check-final.log | cargo test (programs/staking-suite) | **71/71** on FINAL post-fix source; clippy -D warnings clean | yes | step 4 | none |
| build-sbf artifact | PASS | evidence/compile-logs/phase3-sbf-rebuild.log (truncated tail — honest history) + evidence/compile-logs/phase3-sbf-determinism-rerun.log (COMPLETE cold-rebuild re-proof); binaries/ | cargo build-sbf | staking_suite.so 187,504 B, sha256 `57a890fa…3b5564`; **byte-identical on incremental rebuild AND full cold rebuild** (fresh toolchain + fresh platform-tools + empty cache) | yes (same OS/toolchain) | step 5 | none |
| Program ID | PASS (placeholder) / HUMAN ACTION (final) | deployment/scripts/staking-identity.sh; staking-program/PROGRAM-ID-STATUS.md | ./scripts/staking-identity.sh verify | declared `3vEEMM…9mfy` = PRE-DEPLOYMENT PLACEHOLDER; all refs agree; deploy guards refuse mismatch | yes | step 6: generate keypair → set-id → deploy | final ID is buyer's |
| Validator e2e | PASS | evidence/tests/phase5-full-batch.log (+fix logs) | STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1 | **3/3 passed, 160.72 s** — real BPF VM, real mpl clone (mainnet-beta); cap boundaries + funded money flow + metadata + replay rejection | yes (needs internet) | step 7 | none |
| Docker | **BLOCKED** (no daemon) | docker/BLOCKED-NOTE.md; native equivalent in evidence/app-startup/ | docker build / compose config / smoke | native-equivalent binary smoke PASSED (labeled, ≠ container run) | buyer-side | step 19 | sandbox has no Docker |
| CI (GitHub Actions) | **BLOCKED** (no runner) | ci/CI-LOCAL-EQUIVALENCE.md + evidence/gates/ | push → Actions | every CI gate has an executed local 1:1 equivalent; final release-check **20/0/0 exit 0** | local: yes | step 20: record run URL | no runner in sandbox |
| DB backup/restore | PASS | evidence/db/* (dumps, sha256, comparison logs) | pg_dump -Fc → pg_restore --no-owner → psql comparisons → suite rerun | dump sha256 `5989ecf1…`; 24 tables/68 rows/11 migrations IDENTICAL ×3; db_integration **23/23 ON restored DB** | yes | step 18 | none |
| App startup/health/shutdown | PASS | evidence/app-startup/phase8b-*.log | ./sniper-suite + curl battery + SIGTERM | /health ok; /ready 200 (4 components); /api/status paper + live_allowed:false + kill_switch:false; bot_* metrics; clean shutdown | yes | steps 10–11, 13 | none |
| Live/funded validation | WRITTEN / **HUMAN ACTION** | docs/LIVE-VALIDATION.md (package docs/) | procedure only — never auto-executed | Polymarket (balance/decimals/allowance selectors, v2 addresses) + Solana canary procedures; reject-over-fallback pinned by tests | buyer-side | steps 14–16 | real funds + approval required |
| Security evidence | PASS (statics/deps) | evidence/security/* incl. SECURITY-EVIDENCE-INDEX.md (deliverable D) | cargo audit ×2; cargo deny; grep batteries | audit 0 errors (9 allow-listed each); deny advisories/bans/licenses/sources ok; 0 unsafe; forbid ×4; 0 markers; 0 secret-pattern hits; 0 keypairs in tree | yes | step 21 | **no external audit exists — none claimed** |
| Benchmarks | PASS (scope-limited) | evidence/benchmarks/benchmarks-2026-09-18.json + phase11 log | E2E_NETWORK=1 cargo test -p solana-kit --test latency_bench | getSlot n=30 / getLatestBlockhash n=30 / simulate n=10 vs public devnet; landing_rate SKIP (funded); hw/toolchain/network recorded; sandbox figures ≠ product guarantees | yes (values vary) | — | funded leg = HUMAN ACTION |
| SBOM / dependencies | PASS | evidence/sbom/* | sha256sum Cargo.lock*; cargo metadata --locked; cargo tree --locked | app 706 pkgs / staking 580 pkgs; lockfiles `9740cac2…` / `2a00d281…`; no silent dep changes (lockfiles untouched this pass) | yes | step 21 | none |
| Release package | PASS | checksums/SHA256SUMS (deliverable H) | sha256sum -c | **119/119 OK**; 120 files / 11 MB; no secrets/caches/junk (hygiene gate PASS) | yes | step 22 | none |
| Docs consistency | PASS | (working tool doccheck.py; result in gates log context) | python3 doccheck.py | 43 md files, 62 referenced paths, CLEAN | yes | — | none |
| Commercial materials | PASS (factual-only) | docs/ (SELLER-FACT-SHEET, SELLING-LISTING-SOURCE, TECHNICAL-DIFFERENTIATORS, BUYER-OVERVIEW, RELEASE-NOTES updated to hardening facts) | fact-scan + edit batch | no invented customers/revenue/deployments/ROI/guarantees; freeze numbers kept as labeled history | yes | — | none |

## 2. Defects found by execution (compile-green ≠ executed — proven twice)

* **D1 (real program bug):** `processor.rs` metadata discriminant was **19**
  (= `Utilize` in the deployed mpl-token-metadata program); fixed to **33**
  (`CreateMetadataAccountV3`, verified against the deployed program + source
  at tag v1.14.0). Would have failed on-chain at deployment. Proven fixed by
  the executed e2e against the real mainnet-cloned program.
* **D2 (e2e harness bug):** `--clone` cannot clone upgradeable programs;
  fixed to `--clone-upgradeable-program` — the metadata e2e could never have
  run before.
* **D3 (evidence tooling, not repo):** SIGPIPE under `pipefail` killed the
  phase-8b script pre-shutdown; fixed (`{ …; } || true`).
* Also corrected: README's dangerous deploy hint (deploy with the
  build-sbf-generated keypair ≠ declare_id) → safe `staking-identity.sh`
  procedure.

## 3. Exact final counts

| Metric | Value |
|---|---|
| Repo files (final tree, targets excluded) | **175** |
| Bytes / lines | **3,196,164 / 85,164** (after the completeness-audit doc updates) |
| Tree SHA-256 | `51726e7ef038d40ddc17c08d6d43112c321470be7ae4fcc915baa2d6f6bb1630` |
| Files ADDED this pass | 4 (`scripts/staking-identity.sh`, `docs/LIVE-VALIDATION.md`, `docs/BUYER-ACCEPTANCE-TEST.md`, `docs/CI-LOCAL-EQUIVALENCE.md`) |
| Files REMOVED this pass | 0 |
| Files MODIFIED this pass | 20 (2 Rust: processor.rs, validator_e2e.rs; README.md; CHANGELOG.md; release-manifest.json; AUDIT.md §29; 14 docs listed in §29) |
| Docs total | 39 |
| Tests executed this pass | 537 + 537 (AF) + 71 + 3 (e2e) + 23 (restored-DB rerun) + release-check 20/20 gates |
| Tests failed | 0 |
| Tests silently skipped | 0 (gated suites execute with env; skips are loud) |
| Benches executed | 3 legs (getSlot/getLatestBlockhash/simulate); 1 leg SKIP (landing_rate, funded) |
| BLOCKED items | Docker build/compose/smoke (no daemon); GitHub Actions run (no runner) |
| HUMAN ACTION items | final program keypair/ID + deployment; funded live validation (Polymarket + Solana canary); landing-rate bench; external security audit; CI run URL; legal/identity fill-ins |
| Release package | **297 files** / SHA256SUMS 296 entries / self-check **296/296 OK** (incl. full `source-tree/` mirror of all 175 files) |
| Source tarball | `sniper-suite-0.1.0-hardened-src.tar.gz` sha256 `2084d6a9…` (normalized tar) |
| Program .so | 187,504 B, sha256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` (byte-identical rebuild) |

## 4. Deliverables A–H

| # | Deliverable | Location |
|---|---|---|
| A | Release manifest | `buyer-release/BUYER-RELEASE-MANIFEST.json` (+ repo-level `release-manifest.json`) |
| B | Evidence index | `docs/EVIDENCE-INDEX.md` (hardening section) + `evidence/inventory/ledger.csv` (27 EVID entries, per-artifact sha256) |
| C | Buyer acceptance test | `docs/BUYER-ACCEPTANCE-TEST.md` (22 steps + result sheet) |
| D | Security evidence index | `evidence/security/SECURITY-EVIDENCE-INDEX.md` (18 areas + honest gaps) |
| E | Deployment verification | `docs/BUYER-DEPLOYMENT.md`, `docs/DEPLOYMENT.md`, `deployment/` (Dockerfile, compose, templates, migrations, `scripts/staking-identity.sh` guards), acceptance steps 5–6/19 |
| F | Benchmark index | `evidence/benchmarks/benchmarks-2026-09-18.json` (machine-readable) + `phase11-latency-devnet.log` |
| G | DB backup/restore evidence | `evidence/db/` (2 dumps + sha256 + restore/identity/rerun logs) + `docs/BACKUP-RESTORE.md` |
| H | Final release hash manifest | `buyer-release/checksums/SHA256SUMS` (119 entries, `sha256sum -c` = 119 OK) + per-file source inventory |

## 5. Honest limitations (unchanged policy)

No external security audit exists. No mainnet deployment. No funded trading
results. Sandbox bench figures measure this environment, not a production
guarantee. Historical (freeze-era) results remain labeled historical. Local
executions are not GitHub Actions runs. The 0.1.0 snapshot archive
(`/home/user/delivery/`) intentionally still reflects the pre-audit 170-file
tree.

## 6. File-by-file completeness audit (2026-09-19, buyer request: "কোনো ফাইল বাদ যাবে না")

Trigger: buyer asked to verify, file by file and line by line, that nothing was
left out of the delivery and to fix anything missing.

**What was checked (all automated, per file):**

1. **Tarball vs inventory:** 175/175 repo files present in the source tarball,
   0 missing, 0 extra.
2. **Package browsability gap FOUND + FIXED:** 121 files (all `crates/`,
   `programs/` sources, root Cargo/deny/rust-toolchain/.cargo/dotfiles) existed
   ONLY inside the tarball. Added `buyer-release/source-tree/` — a complete
   byte-level mirror of all 175 files, browsable without extraction.
3. **Three-way byte verification:** every file hashed in live repo vs mirror vs
   extracted tarball → **175/175 byte-identical** (`evidence/inventory/package-completeness.csv`,
   175 rows incl. all four hashes + exec-bit columns).
4. **Line-level scan of all 175 files:** empty files, UTF-8 validity, trailing
   newline, CRLF, banned placeholder phrases ("... existing code", "rest
   unchanged", "omitted for brevity", "same as before", "implement later",
   ...), ellipsis-only code lines, TODO/FIXME/todo!/unimplemented!, zero-width/
   bidi characters → **173/175 CLEAN; 2 reviewed false positives** (the
   release-manifest text describing the marker gate, and release-check.sh's own
   grep pattern). Two earlier prose-comment hits (risk.rs:916, pump.rs:1317 —
   "... and once the stop loss level is breached...") were reviewed line-by-line
   and confirmed legitimate test narration, not placeholders. **No real
   placeholder exists anywhere in the delivery.**
5. **Evidence-log integrity sweep:** every log hashed against ledger.csv (all
   MATCH) and tail-checked for completion markers.

**Bugs found and fixed during this audit (honest record):**

* **B1 — truncated evidence log:** `phase3-sbf-rebuild.log` (09-18 incremental
  determinism run) persisted only up to the platform-tools download; the final
  hash-comparison lines existed solely in the non-persisted live monitor.
  FIX: full COLD determinism re-run on 09-19 (reinstalled rust 1.98.1, agave
  2.1.21 tarball hash-verified `5da3359e…`, fresh platform-tools, empty sbf
  cache): `REBUILD_EXIT=0`, .so 187,504 B, SHA-256 `57a890fa…`,
  **`BYTE_IDENTICAL=YES`** vs the preserved first build AND the packaged
  binary — complete log `phase3-sbf-determinism-rerun.log` (ledger EVID-SBF-003).
  The truncated log is KEPT as honest history (labeled superseded), never deleted.
* **B2 — snapshot-mechanism losses (delivery-side, not repo defects):** the
  workspace snapshot wiped `evidence/build/` inside the package (dir name
  "build" is snapshot-excluded) and stripped exec bits workspace-wide
  (scripts, rustup proxies); it also wiped both `target/` dirs and the rust
  toolchain between turns. FIX: package dir renamed `evidence/compile-logs/`;
  exec bits restored (contents hash-verified unchanged); toolchain
  re-provisioned and the cold rebuild above doubles as the re-proof. All
  persisted evidence files (logs, dumps, hashes, package copies) survived
  intact — every ledger hash still matched.
* **B3 — phase8-main.log step-7 clarification:** that log's steps 1–6 PASS
  (dump/restore/IDENTICAL ×3/23-23 on restored DB), but its step-7 app-build
  attempt ended in a transient compile error under disk pressure; the
  successful app-startup evidence is `phase8b-*`. Ledger + AUDIT §29 now state
  this explicitly (the repo index already cited phase8b for app startup — no
  claim was ever misplaced).

**Final package state:** 297 files; `sha256sum -c checksums/SHA256SUMS` =
296/296 OK; source tarball `2084d6a9…`; tree `51726e7e…`; doccheck CLEAN;
nothing missing, nothing extra, no placeholders.


---

## 7. Final buyer-handover pass (2026-09-19) — supersedes counts above

The 22-phase FINAL BUYER-HANDOVER PASS completed after this report was
written. Final state (recomputed, authoritative):

- **Final tree:** 183 files / 3,288,174 bytes / 86,516 lines,
  tree_sha256 `2da6579b6ef20ad628dbe8b9f9f3212465fddf7bb457c0466448925f1190b909`
  (the `51726e7e…` / 175-file figures in §1–§6 describe the hardening-pass
  tree and remain valid as labeled history).
- **Source changes:** exactly one — `scripts/staking-identity.sh` defect fix
  (missing tracked doc + post-set-id sweep false-FAIL on the delivery-time
  manifest); 0 Rust changes, so all recorded test evidence stands.
- **New docs (8):** FEATURE-TRACEABILITY, SECURITY-BOUNDARY-MAP,
  FINAL-IP-AND-THIRD-PARTY-INVENTORY, BUYER-REPRODUCTION-GUIDE,
  FINAL-KNOWN-LIMITATIONS, FINAL-OPERATIONS-HANDOVER, FINAL-INCIDENT-RUNBOOK,
  FINAL-RELEASE-AUDIT (the pass's 23-point closing record).
  BUYER-ACCEPTANCE-TEST upgraded in place to the A–R fill-in format.
- **Final package:** `/home/user/buyer-release-final/` — 314 files,
  SHA256SUMS 312/312 OK, normalized tarball
  `22021e344b6550593eb1671028c476867a5ca1fe3ab66c04543bcdfbdaa4ae7b`
  (rebuild byte-identical), four-way completeness 183/183/183 with 0 diffs,
  provenance pair, manifest restricted to PASS / NOT RUN / BLOCKED /
  HUMAN ACTION. The prior `buyer-release/` package is preserved untouched.


## 8. Independent adversarial audit (2026-09-19, post-packaging)

A 23-rule zero-trust audit re-verified everything in §7 (every hash
recomputed by independent implementations, archive rebuilt, package
byte-compared, secrets scanned, negative tests mapped, identity
fail-closed observed). Fresh executions: verify-delivery 7/7 PASS on the
final tree (pre-fix AND post-fix), tree-hash reproduction MATCH, archive
rebuild BYTE-IDENTICAL, SHA256SUMS second-method 312/312→326/326 after
audit-log inclusion. **5 defects found — all documentation/integrity, 0
Rust, 0 secrets:** D-A1 exec bits stripped by the sandbox snapshot
mechanism (restored ×9); D-A2 incomplete 71/71 evidence citation +
unlabeled pre-fix FAILED e2e logs (citations completed, logs labeled
historical); D-A3 release-check 20/20 lacked tree qualification
(manifest/README/audit now precise: executed on the 175-file hardening
tree, Rust-level gates applicable via byte-identity, not re-runnable
without toolchain); D-A4 stale "175-file three-way" completeness bullet
(updated to final 183-file four-way); D-A5 verify-delivery historical
byte-count nuance (footnoted; log preserved).

**Final authoritative identity (supersedes §7 numbers):** 183 files /
3,293,320 bytes / 86,576 lines, tree_sha256
`033b582f79c5e1d24a301895fa98d37fae1ed01de15f0052a4b2777a6dd06946`;
archive `d0ab6a70deedf6fa17a8e30f02ab86353a0c282ed329f808a8d75acdf1f28ac9`
(880,418 B, rebuild byte-identical); package 328 files, SHA256SUMS 326/326
OK; `.so` unchanged `57a890fa…5564`. Audit logs:
`buyer-release-final/evidence/handover/adversarial-audit/` (+ INDEX.md
pre-fix/post-fix hash caveat) and `/home/user/work/adversarial-audit/`.
