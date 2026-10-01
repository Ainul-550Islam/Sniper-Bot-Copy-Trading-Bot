# AUDIT REMEDIATION — 2026-09-29

> Response to the **2026-09-29 "$20K–$60K BUYER AUDIT"** (now installed verbatim as the repo-root `AUDIT.md`).
> Scope of this round: install the audit, close its immediately-actionable P0 findings (package regeneration + documentation synchronization), and re-verify the release end-to-end on a fresh install of the pinned toolchain.
> Engineering findings that require new code (P0-2 tenant runtime wiring, P0-5 Polymarket V3, remote custody, billing, customer trading UI) remain open backlog — they are tracked, not silently dropped.

## 1. Audit installed verbatim

* `AUDIT.md` (repo root) = the uploaded 2026-09-29 buyer audit, **installed byte-for-byte identical on 2026-09-29** (1,457 lines / 47,796 bytes; verified with `cmp` at installation time). On 2026-09-30, clearly-dated "Post-audit remediation" notes were APPENDED at §13.8, §14.6, §15.5 and §16 — every original finding, number and appendix stands verbatim; the notes only record what was remediated after the audit date (see §8 below).
* No word, heading, number, or appendix was altered, elided, or summarized. The audit's judgments — including every unflattering one — stand as written.
* The prior repo-root `AUDIT.md` (the 54-file-era due-diligence audit, 1,767 lines) was **replaced** by the 2026-09-29 audit as the current document of record. Its content survives unchanged inside the shipped package history and Git history; if a permanent archive copy is wanted at a root-level path, say the word and it will be added without touching `docs/` (which is count-pinned at 101 files).

## 2. P0-1 CLOSED — buyer package regenerated and verified

The audit (§4) found the previously shipped package STALE: manifest DIFFERS, 23 tenant files missing from `buyer-release/source`, 15 files differing, `PROMPT-2-RESULT.md` DIFFERS. Remediation executed:

1. `bash scripts/build-release-package.sh` → PASS (rebuilt from the canonical current tree: 34 migrations, PROMPT-3 sources + tests, the 2026-09-29 `AUDIT.md`, `PROMPT-3-RESULT.md`, refreshed `release-manifest.json`).
2. `bash scripts/verify-buyer-package.sh` → **PASS** (exit 0):
   * every repository file mirrored byte-identical in `buyer-release/source` (4 root supply-chain artifacts relocated by design),
   * `checksums/SHA256SUMS` verifies with `sha256sum -c`,
   * source-tree digest matches `checksums/SOURCE-TREE.sha256` (`1e42ca12834d88c4b994546805e7f2e520316e4596f7c7bf8a6bbfe2e60138f1`),
   * docs parity 101/101 markdown files,
   * 6 evidence/external records present (redacted),
   * SBOM `fd837e42…` and licenses `c1c051ca…` unchanged (no dependency changes — the dependency SBOM is content-stable across source edits).
   * One informational `WARN manifest migration count 34 != 24`: the verifier hardcodes its pre-0025 migration baseline (24); the tree is at 34 after the tenant-isolation migrations. WARN-only by design, not a gate failure. (Baseline update deferred to the script's owner so the verifier's change is deliberate, not slipped in with this remediation.)
3. `bash scripts/verify-delivery.sh` → **7/7 PASS**.

## 3. P0 documentation synchronization CLOSED

* **`docs/FINAL-BUYER-STATUS.md`** — rewritten to the 2026-09-29 truth: 506 Rust files under `crates/` (512 incl. the staking program), 101 docs, 34 migrations, 1,553 `#[test]`; today's evidence block (fresh-toolchain workspace check, all-targets clippy ×8 crates, fmt, saas-sdk 32/32, the four integration harnesses 14/14, bot-core 727/0 against live PostgreSQL, final-release-check ALL PASS); package section now states the 2026-09-29 regeneration and explicitly credits the audit for catching the stale package; added GAP-007 (tenant runtime wiring) so the largest open engineering gap is visible in the status doc itself.
* **`docs/SELLER-FACT-SHEET.md`** — facts synchronized: 8-crate workspace (not 7), 34 forward-only migrations (not 21), current tree measurement (744 files / 512 Rust files / ~196,048 Rust LOC, replacing the 146-file freeze-era measurement), tenant data-plane architecture line, updated test evidence with dated labels, and a rewritten "Current limitations" section that states the two-layer-vs-three-layer tenant truth (control plane + data plane done and isolation-tested; module runtime wiring NOT done), the unimplemented remote custody backends, the manual-only billing provider, and the Polymarket V2/V3 gap — all sourced to the audit's own sections. The "deliberately does NOT say" discipline is preserved and extended (no claim of full multi-tenant runtime isolation).

## 4. Fresh-toolchain re-verification (reproducibility evidence)

The pinned toolchain (`rust-toolchain.toml`: 1.98.1 + rustfmt + clippy, minimal profile) was reinstalled from scratch on a clean sandbox and the full verification stack re-run:

| Gate | Result |
|---|---|
| `cargo check --workspace` (cold) | PASS, 5m01s |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS, 0 warnings (all 8 crates) |
| `cargo fmt --all -- --check` | PASS, 0 diff |
| `cargo test -p saas-sdk` | 32/32 PASS |
| `cargo test -p sniper-suite --test observability_config` | 3/3 PASS |
| `cargo test -p sniper-suite --test release_manifest_integration` | 4/4 PASS |
| `cargo test -p sniper-suite --test buyer_package_integration` | 3/3 PASS (incl. `buyer_release_package_built`) |
| `cargo test -p sniper-suite --test backup_restore_integration` | 4/4 PASS |
| `scripts/final-release-check.sh` | **ALL PASS** (8/8 gates) |
| `scripts/verify-buyer-package.sh` | **PASS** |
| `scripts/verify-delivery.sh` | **7/7 PASS** |

Plus the earlier-in-day live-PostgreSQL evidence (unchanged, on the same pinned versions): bot-core 727/0 across 18 binaries (incl. `db_integration` 26/26 and the 18 cross-tenant isolation tests), tenant data-plane routes 2/2, `tenant_idempotency_isolation` 3/3, manifest enforcement test PASS.

Build discipline for this environment (documented for reproducibility): `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0` for every cargo invocation; 3G swap while linking the heavy `sniper-suite` test binaries.

## 5. Audit findings — disposition table

| Audit ref | Finding | Disposition |
|---|---|---|
| §4 / P0-1 | buyer package stale (manifest differs, 23 missing, 15 differing) | **CLOSED** — regenerated + verified 2026-09-29 (§2 above) |
| P0 doc sync | root AUDIT.md inconsistent; FINAL-BUYER-STATUS + SELLER-FACT-SHEET stale | **CLOSED** — audit installed verbatim; both docs rewritten to current truth (§1, §3) |
| §12 / P0-3 | tenant-scoped trading DB repositories missing | **CLOSED** by PROMPT 3 (pre-audit-snapshot work) — 52-file `trading_repository`, tenant-composite PKs 0026–0034, 18 cross-tenant live-PG tests |
| §12 / P0-4 | atomic arbiter swaps missing | **CLOSED** by PROMPT 3 — 17-surface swap set, zero-downtime `ALTER ... ALTER COLUMN ... TYPE`/shadow-table strategy, STAY-GLOBAL list documented |
| §11 / P0-2 | tenant runtime wiring into modules (spawn_modules process-level; no `OrganizationId` in module crates) | **OPEN — backlog** (GAP-007 in `docs/FINAL-BUYER-STATUS.md`); data plane + repos are tenant-scoped, module layer is not |
| §8/§16 / P0-5 | Polymarket V3 (position IDs, async `tradeIDs`) | **OPEN — backlog** |
| §13 | remote custody (Vault/KMS/HSM) unimplemented, fail-closed | **OPEN — backlog** (fail-closed behavior verified; no silent fallback) |
| §14 | billing synthetic (Stripe/Paddle 501, no self-service checkout) | **OPEN — backlog** |
| §15 | no customer trading dashboard (5-page console only) | **OPEN — backlog** |
| §17–§22 | funded/latency evidence, external audit, legal closeout (LICENSE holder, program ID, trademark) | **OPEN — buyer-action / legal-review** (carried in `docs/FINAL-BUYER-STATUS.md`) |
| §23 | DO-NOT-REWRITE component list | **HONORED** — no component on that list was modified in this round; this round touched only `AUDIT.md`, `docs/FINAL-BUYER-STATUS.md`, `docs/SELLER-FACT-SHEET.md`, this file, and regenerated `buyer-release/` |
| §24 | safe/unsafe marketing claim lists | **HONORED** — the rewritten fact sheet keeps every "does NOT say" boundary and adds the tenant-runtime caveat |
| Appendix A | 45 file/line findings | Each is either closed by PROMPT 3 (tenant/SQL items), closed by this round (docs/package items), or tracked in the open backlog rows above |

## 6. What changed in this round (exact file list)

* `AUDIT.md` — replaced with the uploaded 2026-09-29 audit, verbatim.
* `docs/FINAL-BUYER-STATUS.md` — rewritten (current truth + fresh evidence + GAP-007).
* `docs/SELLER-FACT-SHEET.md` — rewritten (facts synchronized, limitations extended).
* `AUDIT-REMEDIATION-2026-09-29.md` — this report (new).
* `buyer-release/` — regenerated in full (mirror + checksums + manifest copy).
* No Rust source, migration, test, script, or `docs/` count change. `release-manifest.json` unchanged from its PROMPT-3 refresh (counts remain current: 506/1553/34/101).

## 7. Standing rules honored

* Full file contents only — every file written this round is complete; no `# ... existing code ...`-style elision anywhere.
* The audit was installed unmodified — its criticisms of this repository remain in force where still true.
* No claim in any rewritten document exceeds its evidence; every dated label in the status doc corresponds to an actual execution recorded in this conversation's logs.

## 8. Post-round update (2026-09-30, after PROMPT 5)

The table above records this round as it stood on 2026-09-29. PROMPT 5
(2026-09-30) has since closed several of its OPEN rows; statuses below
supersede the table, which is left as written:

| Row | Status now |
| --- | --- |
| §8/§16 P0-5 Polymarket V3 | **CLOSED by PROMPT 5** — V3 EIP-712 path live (`exchange_v3.rs`), position-backed orders (`position_id` XOR-enforced with `token_id`), async `tradeIDs` resolution, backfill, reconciliation; V2 regression green (`PROMPT-5-RESULT.md` §2) |
| §13 remote custody | **CLOSED for Vault/KMS by PROMPT 5** — real transit-engine and SigV4-signed AWS KMS adapters (`crates/server/src/custody/{vault,kms}/`), unit-tested (KMS SigV4 vs the AWS test vector), fail-closed, no local fallback; HSM remains fail-closed unimplemented (`AUDIT.md` §13.8 remediation note; `docs/CUSTODY-STATUS-2026.md`) |
| §14 billing | **CLOSED by PROMPT 5** — Stripe/Paddle adapters, authoritative deterministic billing state, idempotent provider events, checkout, invoices, real store-derived usage limits (`PROMPT-5-RESULT.md` §4, §13-I) |
| §15 customer trading dashboard | **CLOSED by PROMPT 5** — 8 customer pages + 5 components + `customer-trading-api.ts` (operator-global routes unreachable from customer pages) (`PROMPT-5-RESULT.md` §6) |
| §11 P0-2 tenant runtime wiring (GAP-007) | **STILL OPEN (narrowed)** — `TenantExecutionContext` now runs through the trading pipeline and the tenant data plane is tenant-scoped, but the five module crates' startup remains process-level (`docs/FINAL-BUYER-STATUS.md` GAP-007) |
| §17–§22 evidence/legal | **UNCHANGED — buyer-action** |

Docs count is now 109 (this round's "101" references are the 2026-09-29
snapshot). Full acceptance audit A–T: `PROMPT-5-RESULT.md` §13.
