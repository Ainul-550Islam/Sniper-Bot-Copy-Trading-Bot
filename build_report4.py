head = """# sniper-suite 0.1.0 — FINAL DELIVERY BUNDLE & BUYER PRESENTATION PACKAGE Report

Prepared on the completed buyer-handover tree (frozen software: release commit `9c677cd` → freeze commit `0e139c3`; 146 files / 2,801,590 B at freeze). This pass completed the delivery bundle: 9 new documents + 1 bundle-integrity script created; 8 documents finalized/updated. **No trading, risk, staking, schema, migration, auth, signer or execution code was touched — byte-proven in §6.** No git history was fabricated; `.git` remains absent from this sandbox (see §8 and `docs/ARCHIVE-CHECKLIST.md`).

---

## 1. FINAL DELIVERY AUDIT

1. **Starting state verified before edits:** 160 files / 2,919,949 B (post buyer-package), version identity 0.1.0 in all three locations, docs/ = 27.
2. **§17 script justification:** `scripts/verify-delivery.sh` was created because it covers a real gap — `release-check.sh` needs the full toolchain + PostgreSQL + Redis and runs the whole engineering gate, while a received bundle needs a seconds-fast, dependency-light integrity check (required files, version identity, docs count, hygiene, markdown links, invisible characters). It builds/tests nothing and duplicates no release-check gate. It fails closed (missing python3 ⇒ FAIL, any missing file ⇒ FAIL, exit 1). First execution: **7 PASS / 0 FAIL, exit 0**.
3. **§4/§5 finalization:** CAPABILITY-MATRIX rebuilt to the required schema (Capability / Implementation / Source path / Latest verification / Verification type / Known limitation) with no claim upgraded — build-sbf/validator-e2e/devnet/crash-e2e remain PREVIOUSLY VERIFIED, Docker/CI remain NOT EXECUTED, mocks are labeled "VERIFIED (mocks)". BUYER-RISK-REGISTER rebuilt to (Risk / Evidence / Current mitigation / Operational consequence / Buyer action), expanded to 14 rows so every required category is explicit (Geyser/feed-provider and production-infrastructure rows added) + 5 documented non-risks.
4. **§6:** IP-COMPONENTS gained the 15-item Ownership transfer checklist (no credentials or identities invented — every line names an artifact and its state).
5. **§7:** THIRD-PARTY.md reviewed against the required topics (protocol integrations §3, major libraries §2, lockfiles §1, license policy §4, cargo-audit §5, cargo-deny §4, SBOM §6, ownership boundary §3+summary) — **no change needed; left untouched**.
6. **§8:** DELIVERY-MANIFEST now indexes all 23 buyer/delivery docs + the new script, with updated identity counts and reading order starting at FINAL-DELIVERY.md.
7. **§9/§10:** README handover section compacted around the required six links; CHANGELOG gained a second `[Unreleased]` sub-entry; no historical entry rewritten; no technical release fact changed.
8. **§20 git rule honored:** no history recreated, no hashes invented; the authoritative-history requirement (`9c677cd` → `0e139c3`) is documented in FINAL-DELIVERY §2, EVIDENCE-INDEX, ARCHIVE-CHECKLIST, and §8 below.
9. **§21 size rule honored:** growth is documentation + one 7 KB shell script only; no attempt toward 20–30 MB.
10. **Seller-facts discipline:** SELLER-FACT-SHEET and SELLING-LISTING-SOURCE contain explicit non-claim lists (no price/revenue/ROI/clients/volumes/latency guarantees/"enterprise"/audit claims) and mandatory limitation sections.

## 2. FILES CREATED (10 — complete content in §4)

| # | File | Bytes | Lines |
|---|------|-------|-------|
| 1 | docs/FINAL-DELIVERY.md | 13,674 | 220 |
| 2 | docs/BUYER-QUICKSTART.md | 7,762 | 219 |
| 3 | docs/TECHNICAL-FACT-SHEET.md | 8,984 | 151 |
| 4 | docs/SELLER-FACT-SHEET.md | 8,241 | 157 |
| 5 | docs/SELLING-LISTING-SOURCE.md | 9,008 | 169 |
| 6 | docs/DEMO-RUNBOOK.md | 10,831 | 219 |
| 7 | docs/EVIDENCE-INDEX.md | 7,517 | 66 |
| 8 | docs/REPOSITORY-MAP.md | 12,702 | 193 |
| 9 | docs/ARCHIVE-CHECKLIST.md | 5,216 | 106 |
| 10 | scripts/verify-delivery.sh | 7,062 | 155 |

Total created: 90,997 bytes / 1,655 lines.

## 3. FILES MODIFIED (8 — complete final content in §5)

| File | Before → After | Change |
|---|---|---|
| docs/CAPABILITY-MATRIX.md | 9,922 → 11,428 B | Full rewrite to final column schema; 24 capability rows + test-count summary; no status upgraded |
| docs/BUYER-RISK-REGISTER.md | 7,634 → 9,050 B | Full rewrite to final field schema; 12 → 14 risks + 5 non-risks |
| docs/IP-COMPONENTS.md | 10,543 → 13,801 B | Appended "Ownership transfer checklist" (15 items); inventory sections untouched |
| docs/DELIVERY-MANIFEST.md | 7,548 → 10,470 B | Buyer-package table extended with the 9 new docs + script note; identity counts + reading order updated |
| README.md | 29,395 → 29,539 B | Handover section compacted around FINAL-DELIVERY/QUICKSTART/DUE-DILIGENCE/ACCEPTANCE/RISK-REGISTER/DELIVERY-MANIFEST; layout comment 27 → 36 docs; scripts line lists both scripts |
| docs/HANDOVER.md | 7,812 → 8,190 B | §1 bullet: 14 → 23 buyer/delivery docs (+ script disclosure) |
| CHANGELOG.md | 7,348 → 8,442 B | Second `[Unreleased]` sub-entry (final delivery package); historical entries untouched |
| release-manifest.json | 5,420 → 5,420 B | `components.docs_count`: 27 → 36 (single value, same byte length) |

Total modification delta: +10,718 bytes. Created + deltas = 101,715 B = exact tree growth (2,919,949 → 3,021,664).

---
"""

tail = """
---

## 6. CODE-INTEGRITY / BYTE-INTEGRITY RESULT

Frozen baseline (commit `0e139c3`) vs current tree, by measurement:

| Category | Frozen baseline | Current | Verdict |
|---|---|---|---|
| Rust (91 `.rs`: production + tests, app + program) | 2,051,936 B (= 1,826,288 prod + 225,648 tests) | **2,051,936 B** | UNCHANGED (byte-exact) |
| SQL migrations (11 files) | 23,443 B | **23,443 B** | UNCHANGED (byte-exact) |
| All non-markdown files (configs, lockfiles, Docker, CI, scripts, manifest) | 129 files / 2,507,638 B | 130 files / **2,514,700 B = 2,507,638 + 7,062** (the new `verify-delivery.sh`) | UNCHANGED except the one new script (exact arithmetic) |
| Both Cargo.lock files | 201,147 + 160,243 B | identical | UNCHANGED |
| 14 frozen engineering/root docs never edited in any doc pass | 251,616 B | **251,616 B** | UNCHANGED (byte-exact) |
| `cargo fmt --all --check` (pinned 1.98.1) | clean | **clean** | confirms zero Rust drift |

Conclusion: **Rust unchanged, SQL unchanged, migrations unchanged, configs unchanged, deployment unchanged.** The only executable added in any documentation pass is `scripts/verify-delivery.sh`, which builds/tests nothing and is scanned by no release-check gate pattern (gates target `.rs/.sql/.toml/.yml/Dockerfile/.json`).

## 7. DOCUMENTATION CONSISTENCY RESULT

Two independent checkers, both green:

1. **`scripts/verify-delivery.sh` (in-repo, delivered):** 7 PASS / 0 FAIL, exit 0 — required files (58 checked) + migrations 0001–0011 present; version identity `0.1.0` three-way; manifest `docs_count` 36 == actual 36; hygiene clean (no .env/target/build/logs/dumps/keypairs/pem); all relative markdown links across README + 36 docs resolve; no zero-width/bidi characters.
2. **Session checker (`/home/user/doccheck.py`, extended for this pass; deliberately not shipped in the tree):** 40 markdown files, 60 referenced source paths, 170 repo files — `DOC CONSISTENCY: CLEAN`, 0 findings. Adds: stale-count detection (520/518/21 absent from all new docs; HANDOVER's legitimate historical "earlier 518/518" sentence exempt per §18), canonical-count presence (521/521, 23/23, 10/10, 4/4, 1/1, 48/48, 2/2, 20 PASS/0 FAIL/0 SKIP, `9c677cd`, `0e139c3`, 146, 2,801,590), manifest sanity, referenced-path existence, exact file count (170 = 146 + 14 + 9 + 1).

No historical evidence was modified to eliminate a legitimate historical count.

## 8. VERIFICATION RESULTS

| Check | Result |
|---|---|
| `scripts/verify-delivery.sh` | **PASS** — 7/7, exit 0 |
| Documentation consistency checker | **PASS** — CLEAN, 0 findings |
| `cargo fmt --all --check` | **PASS** — pinned toolchain 1.98.1, zero diff |
| `git diff --check` | **NOT EXECUTED** — `.git` absent in this sandbox (infrastructure re-provision). Per §20: no history recreated, no hashes invented. Authoritative history `9c677cd` → `0e139c3` (+ documentation commits) must be preserved in the seller's real repository; the archive procedure is specified in `docs/ARCHIVE-CHECKLIST.md` and explicitly must be run there. |
| `./scripts/release-check.sh` | **NOT RE-RUN (per §22)** — no code/config changed (byte-proven §6); PG/Redis/build cache absent in this sandbox. Authoritative result on identical software bytes: **20 PASS / 0 FAIL / 0 SKIP, exit 0** (freeze run, `AUDIT.md` §27). The new script adds no gate interference (verified against release-check's scan patterns). |
| Full test suite (521 etc.) | **NOT RE-RUN (per §22)** — previous engineering evidence remains authoritative while software bytes are proven unchanged. |

Final delivery tree: **170 files / 3,021,664 bytes (3.02 MB) / 81,583 lines** (frozen software 146 / 2,801,590 / 77,980 + 23 buyer/delivery docs + 1 script + doc edits).

## 9. BUYER-DELIVERY CHECKLIST

- [x] FINAL-DELIVERY.md — single starting point (identity, evidence, taxonomy, actions).
- [x] BUYER-QUICKSTART.md — 18-step hands-on verification, ends before live trading.
- [x] TECHNICAL-FACT-SHEET.md — repository-backed facts only, no adjectives.
- [x] CAPABILITY-MATRIX.md — final schema, 24 capabilities, honest statuses.
- [x] BUYER-RISK-REGISTER.md — 14 risks + 5 non-risks, all required categories.
- [x] IP-COMPONENTS.md — inventory + 15-item ownership transfer checklist.
- [x] THIRD-PARTY.md — reviewed; complete without changes.
- [x] DELIVERY-MANIFEST.md — indexes all 20 required artifacts/docs + script.
- [x] README entry point + CHANGELOG package entries (history untouched).
- [x] SELLER-FACT-SHEET.md + SELLING-LISTING-SOURCE.md — factual, non-deceptive, explicit non-claims.
- [x] DEMO-RUNBOOK.md — 10 deterministic demos, no live trading.
- [x] EVIDENCE-INDEX.md — claim → evidence → status map + re-verification guide.
- [x] REPOSITORY-MAP.md — actual tree, exact counts, nothing invented.
- [x] ARCHIVE-CHECKLIST.md — INCLUDE/EXCLUDE + bundle procedure (to run on the authoritative repo).
- [x] verify-delivery.sh — fail-closed bundle check, executed green.
- [x] Consistency + byte-integrity + fmt verification (§6–8).

## 10. FINAL ARCHIVE CONTENTS (specification — archive produced on the authoritative repo, not here)

INCLUDE: full source (`crates/` + `programs/staking-suite/`), both Cargo.lock files, all 11 migrations, all 36 docs + README/CHANGELOG/AUDIT/SECURITY/LICENSE/VERSION, config examples (`config.toml.example`, `.env.template`, `rust-toolchain.toml`, `deny.toml`, `.cargo/audit.toml`), deployment assets (Dockerfile, compose, .dockerignore, ci.yml), release assets (release-manifest.json, both scripts), and the **real git history** (`git bundle --all` containing `9c677cd` → `0e139c3` + documentation commits).
EXCLUDE: target/, build/, DB dumps/data, Redis data, `.env`, secrets/keypairs/pem, logs, installer temporaries, editor/OS junk.
Full specification + production commands + contamination check: `docs/ARCHIVE-CHECKLIST.md`.

## 11. EXTERNAL/HUMAN REQUIREMENTS (unchanged, authoritative list)

1. Legal copyright holder → `LICENSE`. 2. Real security contact → root `SECURITY.md`. 3. Real repository URL on publish. 4. Staking program deployment + program-id finalization (placeholder today). 5. Independent external security audit before staking mainnet. 6. Production infrastructure (PG ≥ 16, Redis 7, RPC/WS ± Geyser/PumpPortal, hosting, monitoring, secret store, CI runners, Docker daemon, funded keys). 7. First real Docker build+smoke and CI run. 8. Gradual supervised funded live-trading validation. 9. Archive production from the authoritative repository (§10). 10. Credential rotation at transfer (`docs/SUPPORT-HANDOVER.md` §6).

## 12. FINAL HANDOVER INDEX

`docs/FINAL-DELIVERY.md` (start) → `docs/BUYER-QUICKSTART.md` (hands-on) → `docs/DELIVERY-MANIFEST.md` (index of all 36 docs + assets) → `docs/EVIDENCE-INDEX.md` (claim→evidence) → `docs/ACCEPTANCE-CHECKLIST.md` (sign-off) → `docs/ARCHIVE-CHECKLIST.md` (physical bundle) → `release-manifest.json` (machine-readable facts).

Classification at stop: **VERIFIED** — freeze-gate evidence on identical software bytes (521/521, 23/23, 10/10, 4/4, 1/1, 48/48, 20/20, audit/deny/fmt/clippy), byte-integrity proof, both documentation checkers, fmt. **PREVIOUSLY VERIFIED** — build-sbf, validator e2e 2/2, recon_crash_e2e, devnet e2e, latency_bench, ledger replay. **BUYER ACTION REQUIRED** — §11 items 1–5, 8, 10 + acceptance checklist. **EXTERNAL INFRASTRUCTURE REQUIRED** — §11 items 6–7, 9.

**STOP CONDITION MET.** The delivery package is complete; the next stage is the seller/buyer transfer process using the real authoritative repository history — not additional engineering.
"""

dumps = open("/home/user/_dumps.md",encoding="utf-8").read()
full = head + "\n" + dumps + tail
open("/home/user/FINAL_DELIVERY_REPORT.md","w",encoding="utf-8").write(full)
os.remove("/home/user/_dumps.md")
print("report:", os.path.getsize("/home/user/FINAL_DELIVERY_REPORT.md"), "bytes; fences:", full.count("`````"))
