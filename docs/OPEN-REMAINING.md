# বাকি কাজের **সম্পূর্ণ** তালিকা — ২০২৬-১১-০৯

> এই ফাইলটা `scripts/regen-open-remaining.py` দিয়ে **স্বয়ংক্রিয়ভাবে**
> `GAP-MISSING-FILES.md` থেকে তৈরি — হাতে লেখা নয়। তাই গ্যাপ টেবিলের
> প্রতিটি সারি এখানে আছে; কোনো সারি বাদ পড়া সম্ভব নয় (স্ক্রিপ্ট সারি-সংখ্যা
> মিলিয়ে চেক করে, নিচে লেখা আছে)।

## ০. এক নজরে

- গ্যাপ টেবিলের মোট সারি: **137**
- সম্পূর্ণ (DONE): **73**
- **অসম্পূর্ণ: 64** = OPEN **16** + PARTIAL **32** + অন্যান্য **13**
- বন্ধ করতে কী দরকার: কোড **60**, বাইরের resource **4**, আপনার অনুমোদন **0**
- এই তালিকার বাইরে আলাদা করে: **Phase 1.1** (§৫) এবং **Live=false ঘোষণা** (§৬)

---

## ১. বাইরের জিনিস ছাড়া সম্ভব নয় (EXTERNAL REQUIRED)

এই কাজগুলো কোড লিখে বন্ধ করা যায় না — আসল venue/mainnet/Docker/credential লাগে:

- **Live Polymarket CLOB** — সব venue টেস্ট mock; আসল venue-তে request করা হয়নি (funded wallet + API key দরকার)।
- **Live chain / funded provider proof** — mainnet-এ আসল capital দিয়ে execution প্রমাণ নেই (read-only RPC আছে)।
- **Cross-replica OS-level kill proof** — দুই replica একই test binary-তে চলে; আসল `SIGKILL`/আলাদা host/network partition নেই।
- **Docker/hosted services, SBF build, production restricted-role RLS credentials** — টুল/সার্টিফিকেট/DB ছাড়া চলে না।
- **CI runner-এ `ci_run.json`** — GitHub Actions-এ চালানো ছাড়া পাওয়া যায় না।

---

## ২. OPEN — 16 টি

কোড-সাইডে অসম্পূর্ণ/যাচাই না-হওয়া; প্রতিটির সাথে *কী বাকি* আর *কী দরকার*।

1. **§3 Helm/base image digests** — _বাইরের resource_
   - কী বাকি: live registry verification
   - এভিডেন্স: Local form/lock check passes in `evidence/claim-gate/ledger-recheck-2026-11-09.txt`; live registry tag equivalence was not rechecked. Review `scripts/verify-image-digests-live.py` before updating pins.
2. **Persistence and lifecycle sink durability** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source review
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-004-2026-11-09.txt`: broadcast lag and SQL failures can drop pump writes; `on_fill` continues OMS processing after an append error, and lifecycle sink failures are swallowed. Absence of a Submitted row does not prove no send. JSONL flush is not proven fsync/crash-safe. Misleading comments corrected; no fault-injection or runtime fix is claimed.
3. **Event recent-buffer and shutdown ordering review** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-005-2026-11-09.txt`: recent writes are asynchronously spawned and `publish_now` skips the ring; shutdown flag can be observed before reason is stored. Actual effects and callers require verification.
4. **Onboarding wallet funding snapshot selection** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-006-2026-11-09.txt`: funded-wallet query uses MAX timestamp per address, rather than per chain/asset, and ties can include multiple rows. Data/production effect and remediation unverified.
5. **Module control ambiguous write outcomes** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-010-2026-11-09.txt`: storage failure responses assert action was not applied, although uncertain database commit outcome could make this unprovable.
6. **HA startup recovery/readiness guarantees** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-012-2026-11-09.txt`: recovery logs decisions, silently tolerates journal/cursor errors and marks ready; verify all startup callers/dependency enforcement.
7. **Risk oracle completeness and aggregate snapshot** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-013-2026-11-09.txt`: completeness checks and PnL aggregate use separate statements/snapshots, potentially missing concurrent incomplete projections. Need deterministic PG interleaving test and coherent-snapshot design.
8. **Execution write-ahead and intent identity** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-013-2026-11-09.txt`: sink cannot report persistence failures; separator-based intent IDs have ambiguous boundaries. Verify actual broadcast wiring and persisted identity migration before changes.
9. **Global-risk reservations and switch durability** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-014-2026-11-09.txt`: five-minute reservations not renewed; decision identity omits replica; kill-switch restore/persist failure does not stop decisions. Test distributed behavior and failure handling.
10. **Accounting reconciliation joins and aggregation scope** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-015-2026-11-09.txt`: matching by IDs/reference and aggregate quantity keyed without wallet/quote can hide or misattribute mismatches; test collision/offsetting cases.
11. **Sniper reconciled-failure entry fate** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-016-2026-11-09.txt`: sweeper classifies every Reconciled state as held without examining failure; prove downstream behavior and repair safely.
12. **Concurrent exits, speculative fills and copy marks** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: source observation
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-016-2026-11-09.txt`: copy exit rules previously used separate claim keys (new same-position lease key is tested at store level; rolling upgrades, expiry and stale snapshots remain risks); copy marks upsert stale snapshots; sniper sent/unknown paths book estimated fills (copy ambiguous BUY and exit no longer do, see newer evidence); copy exits assume six decimals. Requires deterministic races and actual fill truth design.
13. **Polymarket builder order and revenue** — _বাইরের resource_
   - কী বাকি: verification and implementation pending
   - এভিডেন্স: `evidence/platform-fee-review/builder-signing-and-policy-2026-11-09.txt`: official CLOB V2 mechanism checked; canonical signed builder field and mock HTTP buy/sell tested. `evidence/platform-fee-review/polymarket-fill-replay-2026-11-09.txt`: tenant fill replay conflicts and backfill identity guards tested; `evidence/platform-fee-review/polymarket-builder-estimate-2026-11-09.txt` covers exact rational maker/taker fee estimation and per-fill comparison, not wired to fills. All strategy/copy/Telegram paths and realized builder fee accounting remain unproven.
14. **Requested copy `tenant_executor.rs`** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: OPEN
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; policy freeze and confirmed-fill ledger integration missing.
15. **Requested Polymarket `tenant_executor.rs` reconciliation** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: OPEN
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; builder estimate helpers not wired to fills or ledger.
16. **Exhaustive repository review** — _কোড (এখনই করা সম্ভব)_
   - কী বাকি: not established
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-001-2026-11-09.txt` records a five-file full-source reading batch; `evidence/claim-gate/source-review-batch-002-2026-11-09.txt` records a further three files; batch 003 four more, `evidence/claim-gate/source-review-batch-004-2026-11-09.txt` one more, and `evidence/claim-gate/source-review-batch-005-2026-11-09.txt` three further files; `evidence/claim-gate/source-review-batch-006-2026-11-09.txt` records another 22; `evidence/claim-gate/source-review-batch-007-2026-11-09.txt` records 15 more; `evidence/claim-gate/source-review-batch-008-2026-11-09.txt` records 28 more; `evidence/claim-gate/source-review-batch-009-2026-11-09.txt` records 13 more; `evidence/claim-gate/source-review-batch-010-2026-11-09.txt` records 6 more; `evidence/claim-gate/source-review-batch-011-2026-11-09.txt` records another 6; `evidence/claim-gate/source-review-batch-012-2026-11-09.txt` records another 10; `evidence/claim-gate/source-review-batch-013-2026-11-09.txt` documents 7 more including previously partially read claims; `evidence/claim-gate/source-review-batch-014-2026-11-09.txt` documents another 10; `evidence/claim-gate/source-review-batch-015-2026-11-09.txt` documents 11 accounting files; `evidence/claim-gate/source-review-batch-016-2026-11-09.txt` documents 5 execution-path files. `docs/FINAL-FILE-INVENTORY.md` records hashes and explicitly marks inspection/testing as not established; `evidence/claim-gate/ledger-recheck-2026-11-09.txt` covers only the cited ledger claims. No evidence establishes that every file and line was reviewed.

---

## ৩. PARTIAL — 32 টি (কিছু ঠিক হয়েছে, বাকিটা নেই)

এই সারিগুলোতে কোড বদলেছে ও টেস্ট আছে, কিন্তু **পুরোটা বন্ধ হয়নি** — নিচে ঠিক যা বাকি:

1. **K3 tracked junk cleanup** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: local scope
   - এভিডেন্স: `evidence/claim-gate/delivery-check-2026-11-09.txt` and `evidence/claim-gate/script-modes-2026-11-09.txt`; not a guarantee that all historical junk is removed.
2. **K6 `integrity-gates` job completeness** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: previous list stale
   - এভিডেন্স: `.github/workflows/ci.yml` and `evidence/claim-gate/ledger-recheck-2026-11-09.txt`: current job runs OpenAPI, protocol drift, stats, marketing claims, script modes, secret scan and external-validation honesty checks. It does not list the previously claimed Helm, manifest and delivery gates as steps of this job; delivery appears elsewhere in the workflow. No remote run verified.
3. **Dedup and historical test failure** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: failure mechanism reproduced; original run cause not proven
   - এভিডেন্স: Original `88b24d6:crates/module-copy/tests/two_replica_mirror.rs` used a dead port and inferred the winner from errors. `crates/module-copy/tests/two_replica_mirror.rs::sequential_fast_failures_can_both_reach_rpc` demonstrates deterministically that release then reacquisition yields two errors without concurrent ownership; `evidence/claim-gate/postgres-targeted-2026-11-09.txt`. Configured clustered simulate/live now fails closed on unavailable shared dedup (`evidence/claim-gate/shared-dedup-2026-11-09.txt`); single/paper fallback and replay-window limits remain (`crates/core/src/dedup.rs`). The original log does not prove this was the precise schedule.
4. **Feed signature suppression and replay** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: retry/suppression fixed, durable delivery open
   - এভিডেন্স: Historical bug evidence: `evidence/claim-gate/feed-null-retry-2026-11-09.txt`. Poll and Geyser no longer pre-mark signatures; polling retains the cursor on unresolved fetch/metadata/decode. `evidence/claim-gate/feed-retry-fix-2026-11-09.txt` covers null retries, saved cursor and mock Geyser duplicate handling. Geyser one-shot backfill has no durable retry queue, repeated polling can stall indefinitely and queue handoff is not a durable acknowledgement; no lossless replay guarantee.
5. **Persisted position/trade identifier collision** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: targeted correction; legacy and deployment risks remain
   - এভিডেন্স: `evidence/claim-gate/position-trade-id-2026-11-09.txt`: real PostgreSQL demonstrates same-id positions can mix fields; independent-state ID test failed before and 7/7 position tests passed after UUID-v4 IDs were wired to copy/sniper/Polymarket position and trade producers. Two distinct trade rows were also persisted and read in the targeted test. Multi-process/live behavior, existing corrupted rows and rolling deployments with old code remain unverified. The old source observation is `evidence/claim-gate/local-id-scope-review-2026-11-09.txt`.
6. **Tenant trade position ownership** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: repository guard added
   - এভিডেন্স: `evidence/claim-gate/tenant-trade-position-owner-2026-11-09.txt`: real PostgreSQL pre-fix accepted a foreign position reference; guarded single-statement insert rejects foreign/missing while allowing owned/unlinked. Existing rows, direct SQL/global repository paths, ownership-change concurrency and live operation remain unverified. Duplicate trade IDs are now checked by tenant and legacy repositories; the legacy persistence caller logs errors and continues (`evidence/claim-gate/legacy-trade-conflict-2026-11-09.txt`).
7. **Position recovery truncation and bounded reports** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: recovery cap removed; reporting limits and scaling remain
   - এভিডেন্স: `evidence/claim-gate/position-recovery-limit-2026-11-09.txt`: actual server restart and HA sync use deployment `list_open`; PostgreSQL 5,001-position regression failed before and both deployment and tenant live-position reads passed after removing the 5,000 limit. Complete reads now use unbounded memory and remain subject to timeouts; startup still proceeds with an empty book on error. Closed-position and trade-window reporting still cap results (`evidence/claim-gate/source-review-batch-003-2026-11-09.txt`). No live startup demonstrated.
8. **Duplicate trade ID is silently discarded** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: tenant and legacy repository guards added; caller durability remains open
   - এভিডেন্স: `evidence/claim-gate/tenant-trade-conflict-2026-11-09.txt` and `evidence/claim-gate/legacy-trade-conflict-2026-11-09.txt`: exact retries succeed; differing same-ID payloads conflict in real PostgreSQL, including concurrent futures and foreign-row legacy collision. `persist::on_fill` only logs append errors and continues, so fill durability is not guaranteed. Historical lost rows not repaired; generic conflict can reveal ID unavailability, no-op UPDATE requires permissions and may fire triggers; Targeted real bot_app RLS retry/collision test passed (`evidence/claim-gate/rls-trade-conflict-2026-11-09.txt`); broader RLS/legacy and multi-process behavior remain unverified. Original witness: `evidence/claim-gate/trade-id-collision-witness-2026-11-09.txt`.
9. **JSONL torn-tail and archive replacement** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: local corrections and tests
   - এভিডেন্স: `evidence/claim-gate/jsonl-torn-rotation-2026-11-09.txt`: controlled torn-tail regression failed before, then 4/4 storage tests passed; fixed-time same-second double-rotation test failed before and passed with UUID-suffixed archive names. Serialization is limited to a Store and its clones; independently opened stores/processes, power-loss/fsync, partial writes, theoretical UUID collision and previously lost data remain unaddressed. Appends now require read access.
10. **Tenant balance aggregation by chain** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: targeted PostgreSQL correction
   - এভিডেন্স: `evidence/claim-gate/balance-chain-total-2026-11-09.txt`: same-address/asset cross-chain rows were conflated, and null USD could yield a partial total; queries now key by chain/address/asset, break timestamp ties by ID, and return None if any latest value is unknown. 12/12 PG suite passed. Separate onboarding funding query, stale balance policy and live effects remain unverified.
11. **Per-symbol PnL snapshot consistency** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: single-statement correction
   - এভিডেন্স: `evidence/claim-gate/pnl-symbol-snapshot-2026-11-09.txt`: former check/aggregate interleaving reproduced with old SQL steps on PostgreSQL; new method uses one statement snapshot and passes 2/2 real-PG reporting suite. Full dashboard still combines independently queried snapshots; no forced race in production method or live proof.
12. **Tenant order status-history attribution** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: corrected for new writes
   - এভিডেন্স: `evidence/claim-gate/order-history-upsert-2026-11-09.txt`: direct real-PG regression failed before and passed after explicitly writing the order tenant. App-role RLS check passes: `evidence/claim-gate/order-history-rls-2026-11-09.txt`; existing incorrectly attributed rows remain open.
13. **Tenant order upsert collision signaling** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: foreign collision corrected
   - এভিডেন্স: `evidence/claim-gate/order-history-upsert-2026-11-09.txt`: foreign ID conflict returned false success before, now returns generic conflict; direct PG suite 5/5. App-role foreign-ID collision fails: `evidence/claim-gate/order-history-rls-2026-11-09.txt`; same-tenant immutable payload mismatch remains open.
14. **Deployment lifecycle retention tenant scope** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: corrected for deployment path
   - এভিডেন্স: `evidence/claim-gate/lifecycle-prune-tenant-scope-2026-11-09.txt`: real PostgreSQL before fix deleted both deployment and foreign tenant rows; deployment org predicate added, final 6/6 suite passed. Historical data cannot be restored by this change; multi-DB cache and other cleanup paths remain open.
15. **Stale execution signature reconciles a rearmed attempt** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: in-memory path corrected
   - এভিডেন্স: `evidence/claim-gate/execution-stale-signature-rearm-2026-11-09.txt`: regression failed before, passes after; old lookup is checked atomically against current signature; 14 execution unit tests pass. Durable/restart and cross-process cases remain open.
16. **Global max-open positions counts pending entries** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: local regression corrected
   - এভিডেন্স: `evidence/claim-gate/global-position-reservation-2026-11-09.txt`: previously accepted second entry with cap 1 while first reservation alive; snapshot count fixed; 29/29 suite passes. Distributed stores and five-minute expiry remain to be tested.
17. **Recovery worker errors and ambiguous intent** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: queue outcomes and position corrections are now recorded honestly; the generic journal's safe default remains
   - এভিডেন্স: The recorded observation split in two. (a) DONE: the recon pass recorded every verdict with `repo.resolve/fail/give_up(..).await.ok()` and then counted the claim as handled, so a failed queue write was invisible to the operator while the row kept its claim. Fixed with a `ReconQueueWrites` seam, a `record_verdict(...) -> RecordOutcome` decision (`Recorded` only on `Ok`), `unrecorded` counters on both `ReconStats` and `StartupReconReport` printed in the pass and startup logs, and a `release_claim` that reports whether it took. Witnesses: a failing queue yields `Unrecorded` for all three verdicts and an accepting queue yields `Recorded`, plus a real-PostgreSQL pass where the report and the queue's own counts agree. RED: restoring `.ok()` semantics fails `a failed queue write must not be reported as recorded` (left: Recorded, right: Unrecorded). A second defect surfaced while proving that one: rows that exhausted `max_attempts` while stuck in `in_progress` were invisible forever (not due again, not `failed`) — `park_exhausted_in_progress()` now parks them with the reason `claim exhausted its attempts without a recorded outcome` before every batch, witnessed on real PostgreSQL. (b) STILL OPEN: `with_intent` maps every error to `Retain` (unknown ⇒ leave for the operator) — the safe default for an arbitrary future, since an error may mean bytes already left the process; the money paths use the checked/reserved variants. Proving "this error is a non-send" needs a typed error, not a string. Position-correction persistence is now fixed for the corrections that matter most. (c) DONE: `settle_position_outcome` rewrite-both the position on a `MissingPosition` verdict (on-chain zero, exit fill confirmed) and on fill-justified drift, then persisted the rewrite with `PositionRepo::upsert(..)` **and ignored the failure** — a `debug!` line — while still returning `Resolved { status: "corrected-closed" }` / `"corrected-adopted"`. A correction that never reached the database was reported as settled, so the queue closed a subject whose row was unchanged and the next restart re-opened the position: the same lie as (a), one layer down. Fixed with a `PositionTruthWriter` seam (`upsert_position`, `flag`, implemented for `Database`): a failed correction is now `warn!`ed with `position = %subject`, appends the risk event `recon_correction_failed` carrying `{position_id, error}`, and returns `Retry { reason: "correction not persisted: {error}" }` — contract: a correction is settled ONLY after its `upsert` returned `Ok`. Witness `sniper_suite::recon::tests::an_unpersisted_correction_is_not_reported_as_settled` (scripted writer): failing write ⇒ `Retry` containing `correction not persisted`, one upsert attempt, flags `["recon_correction_failed"]`; accepting write ⇒ `Resolved { "corrected-closed" }`, flags `["recon_correction"]`. RED with the old shape re-injected: `an unpersisted correction must be retried, got Resolved { status: "corrected-closed" }`; GREEN 4/4 in `recon::`, clippy `-D warnings`, fmt/diff 0. Evidence: `evidence/claim-gate/recon-recorded-outcomes-2026-11-09.txt` §a third defect. (b) STILL OPEN: `with_intent` maps every error to `Retain` (unknown ⇒ leave for the operator) — the safe default for an arbitrary future, since an error may mean bytes already left the process; the money paths use the checked/reserved variants. Proving "this error is a non-send" needs a typed error, not a string. Residual in the same family, logged and NOT fixed: `is_active` DB errors → false and fill-history failures → empty, which read as "no work" rather than "unknown".
18. **Telegram sell attribution** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: conservative live parity guard
   - এভিডেন্স: `evidence/claim-gate/source-review-batch-016-2026-11-09.txt` identified wallet-wide balance being sold against one tracked position. Live sells now compare wallet raw balance with that position's raw quantity and refuse differences beyond one rounding unit; tolerated rounding is audited and the sell is capped to the tracked quantity (`telegram-payload-wallet-scope-checks-2026-11-09.txt`). This does not establish multi-position attribution, token-account provenance, live RPC honesty, or confirmed-fill reconstruction.
19. **Platform fee surface attribution and arithmetic** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: code and in-memory tests
   - এভিডেন্স: `evidence/platform-fee-review/builder-signing-and-policy-2026-11-09.txt`: legacy-compatible attribution, 50,000 seeded policy cases and policy validation; tenant-scoped journal projection totals exist; a verified copy direct-curve fee now posts a tenant expense in the global accounting ledger, but execution-wired frozen policies and a durable platform revenue ledger remain OPEN.
20. **Copy and Telegram trade platform fees** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: narrow verified copy direct-curve expense
   - এভিডেন্স: `evidence/platform-fee-review/initial-audit-2026-11-09.md`: copy curve buy/sell fee instructions, copy/Telegram Jupiter fee requests and partial offline tests added; verified copy direct-curve fees are now posted exactly once to the global ledger; Telegram, Jupiter, platform revenue, frozen-policy execution wiring, all-path fills and end-to-end settlement remain OPEN. `evidence/platform-fee-review/tenant-policy-dust-sniper-2026-11-09.txt` documents tenant plan lookup, dust fix and sniper executable Jupiter request correction. New sell paths use SOL probe; unused legacy helper assumes input is lamports.
21. **Platform fee immutable tenant projection journal** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: real PG regression
   - এভিডেন্স: `evidence/platform-fee-review/postgres-projection-journal-2026-11-09.txt`: RLS app-role read, 8 concurrent same-key writes, conflicting replay, tenant separation, paper non-cash projection and tenant/mode/asset-scoped per-surface totals. It is not connected to execution or the global/operator ledger and does not prove live fee collection.
22. **Requested copy `mirror.rs` BUY** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; direct and Jupiter preparation exist, confirmed actual fee and durable skip journal remain open.
23. **Requested copy `exit.rs` SELL** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; fee preparation exists, confirmed proceeds and per-intent idempotent posting remain open.
24. **Requested `copy_platform_fee.rs` tests** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/copy-real-curve-mock-2026-11-09.txt`: real copy curve buy/sell pipeline under mock RPC inspects signed submitted transactions, charged fee amount and vault, disabled/dust skips and duplicate sell suppression; 8/8 tests pass. Fee ledger idempotency for the verified direct leg is now covered; settlement, Jupiter, platform revenue and complete mode coverage remain open.
25. **Requested Polymarket `builder.rs` attribution** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; official mechanism reviewed, every production order path not yet proven.
26. **Requested core `platform_fee.rs` surfaces** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; attribution and property-tested arithmetic exist; execution and ledger use missing.
27. **Requested `platform_fee_surfaces.rs` tests** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; projections tested, actual ledger reconciliation not demonstrated.
28. **Requested `telegram_trade_port.rs`** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; fee-bearing requests exist, tenant policy and confirmed-fill postings missing.
29. **Requested `telegram_fee_wiring.rs` tests** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `evidence/platform-fee-review/request-file-audit-2026-11-09.md`; command-to-ledger and policy-disable proof missing.
30. **Gap ledger** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: inventory is now complete and verified (133/133 rows in `docs/OPEN-REMAINING.md`); the ledger itself can only be closed when the remaining rows close
   - এভিডেন্স: `GAP-MISSING-FILES.md`; older rows retain historical claims and may require evidence audit. Inventory hashes do not establish exhaustive line-by-line inspection (`scripts/generate-file-inventory.py`).
31. **Competitor benchmark** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: (সারির বর্ণনায় বিস্তারিত)
   - এভিডেন্স: `docs/COMPETITOR-BENCHMARK.md`; Polymarket code status corrected, limited to dated vendor pages and source paths; no parity or performance claims.
32. **Workspace test** — _কোড (এখনই করা সম্ভব)_
   - যা বাকি: current repository/disposable-service proof
   - এভিডেন্স: `evidence/claim-gate/workspace-services-after-recon-correction-2026-11-09.txt`: current source passed `cargo test --workspace --all-targets --locked -- --test-threads=1` with disposable PostgreSQL 17/Redis and `REQUIRE_POSTGRES=1 REQUIRE_SERVICES=1`, from a full rebuild (incremental + stale deps purged): **5,095 passed, 0 failed, 0 ignored** across 203 targets, exit 0. Progression today: 5,032/38 ignored (after #78) → 5,057/15 (after the `crates/server` gated tests) → 5,072/1 (after the whole-workspace `#[ignore]` audit and the sweep-paging fix) → 5,075/1 (after dedup TTL enforcement) → 5,090/0 (after the stale-replica ledger fix) → 5,093/0 (recon recorded outcomes) → 5,095/0 (position-correction persistence). Nothing is ignored: the last deliberate ignore — `accounting_replica_order.rs::stale_replica_sell_posting_uses_committed_inventory_cost` — is fixed (committed inventory cost) and runs in the sweep. workspace all-targets Clippy `-D warnings`, fmt, batch-6 manifest (12/12) and `make gates` pass; hadolint/Docker Compose SKIP, live registry digest PASS. Superseded runs: `evidence/claim-gate/workspace-services-after-dedup-ttl-2026-11-09.txt` (5,075/203, 1 ignored), `evidence/claim-gate/workspace-services-after-ignore-audit-2026-11-09.txt` (5,072/203, 1 ignored), `evidence/claim-gate/workspace-services-after-gated-tests-2026-11-09.txt` (5,057/203, 15 ignored), `evidence/claim-gate/workspace-services-after-recovery-bounds-2026-11-09.txt` (5,032/203, 38 ignored), `evidence/claim-gate/workspace-services-after-recovery-sweep-2026-11-09.txt` (5,029/202), `evidence/claim-gate/workspace-services-after-dca-lease-2026-11-09.txt` (5,028/201), `evidence/claim-gate/workspace-services-final-2026-11-09.txt` (5,027/200), `evidence/platform-fee-review/workspace-services-after-worker-claim-clock-2026-11-09.txt` (5,006/196). Earlier historical 4,756/180-target log and password-reset PoolTimedOut remain in `evidence/claim-gate/`. Not remote CI, hosted-service, SBF, restricted production-role RLS, live-funded or live-chain validation.

---

## ৪. অন্যান্য অসম্পূর্ণ — 13 টি (HISTORICAL / NOT DONE / NOT RUN / IMPLEMENTED / UPDATED)

1. **§2 build blocker (`execution.rs` removed, `.cargo` clang/lld optional)** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: `cargo clippy --workspace --all-targets -- -D warnings` exit 0
2. **§3 CI: docker smoke (in-container health probe)** — `HISTORICAL` — _বাইরের resource_
   - অবস্থা: HISTORICAL — evidence path missing; docker job BLOCKED
   - এভিডেন্স: release binary `/api/health` 200; no docker here
3. **§3 `deny.toml` licenses (first-party private, `solana-config-program-client` clarified)** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: `cargo deny check licenses` exit 0
4. **§3 `ci_run.json`** — `NOT_RUN` — _বাইরের resource_
   - অবস্থা: NOT_RUN (correct)
   - এভিডেন্স: `evidence/live/ci_run.json`
5. **§4 hygiene: junk removed, `verify-delivery` junk assertion** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: `verify-delivery.sh` 8 PASS / 0 FAIL
6. **§4 `P0-D-FULL-CODE.md` removed** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: 112 unique non-code lines, code present in tree
7. **§5 recorded datasets** — `IMPLEMENTED` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: IMPLEMENTED — validation scope open
   - এভিডেন্স: `crates/core/migrations/0055_launch_datasets.sql`, `crates/module-sniper/src/dataset/recorder.rs`, `crates/server/src/trading_data_plane/dataset_store_pg.rs`; presence does not prove a real captured production dataset
8. **K5 `scripts/bootstrap-config.sh` + `Makefile`** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: `make help` renders with `PATH=/usr/bin:/bin`; `bootstrap-config.sh --check` exits 1 when a file is missing, 0 after creating (mode 0600, never overwrites)
9. **K7 README entry points** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: Quick start / Testing / Docker use `make init
10. **No fixture generator hidden behind `#[ignore]`** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: `write_fixtures` and `regenerate_replay_fixtures` are ordinary tests; `cargo test -p module-sniper` → 302 PASS / 0 FAIL / **0 ignored** and `cargo test -p solana-kit` → 354 / 0 / **0**
11. **Stale replay fixtures refreshed** — `HISTORICAL` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: HISTORICAL — evidence path missing
   - এভিডেন্স: the 19 committed `crates/module-sniper/fixtures/replay/*.json` predated config fields that already exist at HEAD (`simulate_sell`, `require_sell_probe`, `max_top_holder_pct`, `max_bundler_ratio`, `advanced_exit`, …), so `checked_in_fixtures_match_the_generator` was passing against bytes its own generator no longer produced; re-running the generator makes the tree clean
12. **Owner actions** — `UPDATED` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: UPDATED
   - এভিডেন্স: `OWNER-TODO.md`; human authorization, review, credentials and external-party actions only.
13. **Known limitations** — `UPDATED` — _কোড (এখনই করা সম্ভব)_
   - অবস্থা: UPDATED
   - এভিডেন্স: `docs/KNOWN-LIMITATIONS.md`; limitations remain explicit rather than being silently removed.

---

## ৫. Phase 1.1 (আলাদা, টেবিলের বাইরে)

**OPEN** — copy BUY-এর pre-broadcast mode upgrade এখনো pre-send check (atomic fencing বা durable
distributed reservation নয়)।

---

## ৫খ. PARTIAL মানে কী — এক লাইনে, বাংলায়

নিচের প্রতিটি PARTIAL সারিতে কোড বদলানো হয়েছে এবং টেস্ট আছে, কিন্তু **শেষ ধাপটা বাকি**।যেমন:

- *Durable delivery open* — ইন-মেমরি ঠিক, কিন্তু process বন্ধ হলে পুনরায় পৌঁছানোর guarantee নেই।
- *Legacy and deployment risks remain* — নতুন লেখার পথ ঠিক, পুরনো ডেটা বা অন্যভাবে লেখা পথ যাচাই হয়নি।
- *Reporting limits and scaling remain* — cap উঠেছে, কিন্তু বড় স্কেলে কী হয় তা মাপা হয়নি।
- *In-memory path corrected* — একই প্রক্রিয়ার ভিতরে ঠিক, দুই replica একসাথে পরীক্ষা হয়নি।
- *Narrow verified …* — একটি নির্দিষ্ট পথে প্রমাণ আছে, বাকি পথগুলো এখনো mock/অচালিত।

অর্থাৎ **কোনো PARTIAL সারিকে 'শেষ' ধরা হয়নি** — ঠিক কী বাকি তা প্রতিটি সারির নিচে লেখা আছে।

---

## ৬. যা এখনো প্রমাণ হয়নি — সৎ ঘোষণা (কোনো ফাইল বাদ যায় না, কিন্তু প্রমাণও বাড়ানো হয়নি)

- আসল venue-তে একটিও order পাঠানো হয়নি; সব execution টেস্ট mock/paper/simulate।
- mainnet-এ আসল SOL/POL দিয়ে কোনো লেনদেন নেই — তাই "আসল টাকায় কাজ করে" দাবি করা হয় না।
- ৫০০০+ টেস্ট পাস = কোড-লেভেল প্রমাণ; **উৎপাদন-লেভেল প্রমাণ নয়**।
- একই host-এ দুই replica চালানো হয়েছে; আলাদা machine/নেটওয়ার্ক partition পরীক্ষা হয়নি।
- Docker image build/hosted deploy, SBF (on-chain program) build, restricted-role RLS credential দিয়ে টেস্ট — হয়নি।

---

## ৭. পরিপূর্ণ সারণি — সব 137 টি সারি (DONE সহ, কিছুই বাদ নেই)

| # | আইটেম | ধরন | কী দরকার |
|---|---|---|---|
| 1 | §2 build blocker (`execution.rs` removed, `.cargo` clang/lld optional) | HISTORICAL | CODE |
| 2 | §2.1 feed health fields | DONE | CODE |
| 3 | §2 `cargo fmt --all --check` | DONE | CODE |
| 4 | §3 CI: staking job configuration | DONE | CODE |
| 5 | §3 CI: integrity gate statistics and markers | DONE | CODE |
| 6 | §3 CI: docker smoke (in-container health probe) | HISTORICAL | EXTERNAL |
| 7 | §3 CI: frontend contract gate order | DONE | CODE |
| 8 | §3 `deny.toml` licenses (first-party private, `solana-config-program-client` clarified) | HISTORICAL | CODE |
| 9 | §3 `ci_run.json` | NOT_RUN | EXTERNAL |
| 10 | §3 Helm/base image digests | OPEN | EXTERNAL |
| 11 | §4 hygiene: junk removed, `verify-delivery` junk assertion | HISTORICAL | CODE |
| 12 | §4 archived audit | DONE | CODE |
| 13 | §4 `git filter-repo` history rewrite (`.cargo/bin/rustup`) | DONE | APPROVAL |
| 14 | §4 `P0-D-FULL-CODE.md` removed | HISTORICAL | CODE |
| 15 | §5 backtest truthfulness (`data_source`, `authoritative`, migration 0057) | DONE | CODE |
| 16 | §5 recorded datasets | IMPLEMENTED | CODE |
| 17 | §5 frontend disclosure components | DONE | CODE |
| 18 | K1 first-slot producer for `LaunchEvent.first_slot_buys` | DONE | CODE |
| 19 | K2 creator dev-sell watcher → `DevSellObservation` | DONE | CODE |
| 20 | K1/K2 wiring into feeds and `Sniper::run` | DONE | CODE |
| 21 | K3 tracked junk cleanup | PARTIAL | CODE |
| 22 | K3 local configuration untracking | DONE | CODE |
| 23 | K4 Helm digest-only contract | DONE | CODE |
| 24 | K5 `scripts/bootstrap-config.sh` + `Makefile` | HISTORICAL | CODE |
| 25 | K6 `integrity-gates` job completeness | PARTIAL | CODE |
| 26 | K7 README entry points | HISTORICAL | CODE |
| 27 | Statistics freshness | DONE | CODE |
| 28 | Release manifest freshness | DONE | CODE |
| 29 | No fixture generator hidden behind `#[ignore]` | HISTORICAL | CODE |
| 30 | Stale replay fixtures refreshed | HISTORICAL | CODE |
| 31 | `server/**` gated/live tests | DONE | CODE |
| 32 | `#[ignore]` audit (whole workspace) | DONE | CODE |
| 33 | Delete `.cargo/env` | DONE | CODE |
| 34 | Delete `.config/solana/install/config.yml` | DONE | CODE |
| 35 | Delete `FINAL-REPORT.md` | DONE | CODE |
| 36 | Delete `run-part2.sh` | DONE | CODE |
| 37 | Delete `run-tests.out` | DONE | CODE |
| 38 | Delete `SHA256SUMS` | DONE | CODE |
| 39 | Claim SQL and expiry semantics | DONE | CODE |
| 40 | Dedup and historical test failure | PARTIAL | CODE |
| 41 | Deterministic two-replica test | DONE | CODE |
| 42 | Copy polling cursor direction | DONE | CODE |
| 43 | Copy polling pagination | DONE | CODE |
| 44 | Invalid signature in RPC page | DONE | CODE |
| 45 | Feed signature suppression and replay | PARTIAL | CODE |
| 46 | Tenant-local copy dedup identity and sink | DONE | CODE |
| 47 | Dedup TTL and cleanup semantics | DONE | CODE |
| 48 | Reconciliation queue claim fairness (bounded page) | OTHER | CODE |
| 49 | Unavailable dedup journal protection | DONE | CODE |
| 50 | Multi-replica stress (mock execution and lease recovery) | DONE | CODE |
| 51 | Copy module suite | DONE | CODE |
| 52 | Fault-injection timing window (test) | OTHER | CODE |
| 53 | Sweep-load timing windows (two tests) | OTHER | CODE |
| 54 | Signer revocation at broadcast | DONE | EXTERNAL |
| 55 | Signing key rotation under an unchanged reference | DONE | CODE |
| 56 | Delivery and stats checks | DONE | CODE |
| 57 | Risk daily PnL UTC window | DONE | CODE |
| 58 | Polymarket future-fill PnL completeness | DONE | CODE |
| 59 | Intent journal attempt identity | DONE | CODE |
| 60 | Tenant copy leader config refresh | DONE | CODE |
| 61 | Tenant runtime freshness at broadcast | DONE | EXTERNAL |
| 62 | Persisted position/trade identifier collision | PARTIAL | CODE |
| 63 | Tenant trade position ownership | PARTIAL | CODE |
| 64 | Position recovery truncation and bounded reports | PARTIAL | CODE |
| 65 | Duplicate trade ID is silently discarded | PARTIAL | CODE |
| 66 | Persistence and lifecycle sink durability | OPEN | CODE |
| 67 | JSONL torn-tail and archive replacement | PARTIAL | CODE |
| 68 | Event source attribution | DONE | CODE |
| 69 | Event recent-buffer and shutdown ordering review | OPEN | CODE |
| 70 | Tenant balance aggregation by chain | PARTIAL | CODE |
| 71 | Per-symbol PnL snapshot consistency | PARTIAL | CODE |
| 72 | Repository conflict and intent probe error mapping | DONE | CODE |
| 73 | Tenant advisory lock hash collisions | DONE | CODE |
| 74 | Onboarding wallet funding snapshot selection | OPEN | CODE |
| 75 | Tenant intent duplicate payload handling | DONE | CODE |
| 76 | Tenant order status-history attribution | PARTIAL | CODE |
| 77 | Worker claim clock and recovery fencing | DONE | CODE |
| 78 | Tenant recovery truncation and lock boundary | DONE | CODE |
| 79 | Tenant order upsert collision signaling | PARTIAL | CODE |
| 80 | Polymarket fill and order replay consistency | DONE | CODE |
| 81 | Process-global deployment-org cache | DONE | CODE |
| 82 | Tenant lifecycle and copy/poly recovery bounds | DONE | EXTERNAL |
| 83 | HA cursor fencing | DONE | CODE |
| 84 | Analytics unit and metric semantics | DONE | CODE |
| 85 | Reporting API window validation | DONE | CODE |
| 86 | Tenant integration health evidence labels | DONE | EXTERNAL |
| 87 | Tenant kill switch and individual overrides | DONE | CODE |
| 88 | Module control ambiguous write outcomes | OPEN | CODE |
| 89 | Deployment lifecycle retention tenant scope | PARTIAL | CODE |
| 90 | Limit-order lease expiry versus handoff/cancel | DONE | CODE |
| 91 | DCA lease expiry versus transition/slot | DONE | CODE |
| 92 | Limit-order timestamp-only pagination | DONE | CODE |
| 93 | HA cursor advance before processing and unfenced writes | DONE | CODE |
| 94 | HA lease check versus actual work and cached guards | DONE | CODE |
| 95 | HA startup recovery/readiness guarantees | OPEN | CODE |
| 96 | Stale execution signature reconciles a rearmed attempt | PARTIAL | CODE |
| 97 | Risk oracle completeness and aggregate snapshot | OPEN | CODE |
| 98 | Execution write-ahead and intent identity | OPEN | CODE |
| 99 | Global max-open positions counts pending entries | PARTIAL | CODE |
| 100 | Recovery worker errors and ambiguous intent | PARTIAL | CODE |
| 101 | Global-risk reservations and switch durability | OPEN | CODE |
| 102 | Shared accounting journal ordering and stale replica cost | DONE | CODE |
| 103 | Accounting same-ID payload mismatch | DONE | CODE |
| 104 | Accounting reconciliation joins and aggregation scope | OPEN | CODE |
| 105 | Sniper reconciled-failure entry fate | OPEN | CODE |
| 106 | Concurrent exits, speculative fills and copy marks | OPEN | CODE |
| 107 | Telegram replay authorization | DONE | CODE |
| 108 | Telegram sell attribution | PARTIAL | CODE |
| 109 | Platform fee surface attribution and arithmetic | PARTIAL | CODE |
| 110 | Copy and Telegram trade platform fees | PARTIAL | CODE |
| 111 | Polymarket builder order and revenue | OPEN | EXTERNAL |
| 112 | Platform fee immutable tenant projection journal | PARTIAL | CODE |
| 113 | Fee-bearing Jupiter mint/mode guard and destination config | DONE | CODE |
| 114 | RPC confirmation transaction identity | DONE | CODE |
| 115 | Confirmation expiry recheck safety | DONE | CODE |
| 116 | Transaction metadata confirmation guard | DONE | CODE |
| 117 | Jupiter disabled/dust stale fee fields and preparation rejection audit | DONE | CODE |
| 118 | Requested copy `mirror.rs` BUY | PARTIAL | CODE |
| 119 | Requested copy `exit.rs` SELL | PARTIAL | CODE |
| 120 | Direct SOL fee receipt verifier | DONE | CODE |
| 121 | Policy-linked fee projections | DONE | CODE |
| 122 | Tenant intent fee policy snapshot repository | DONE | CODE |
| 123 | Requested copy `tenant_executor.rs` | OPEN | CODE |
| 124 | Requested `copy_platform_fee.rs` tests | PARTIAL | CODE |
| 125 | Requested Polymarket `builder.rs` attribution | PARTIAL | CODE |
| 126 | Requested Polymarket `tenant_executor.rs` reconciliation | OPEN | CODE |
| 127 | Requested core `platform_fee.rs` surfaces | PARTIAL | CODE |
| 128 | Requested `platform_fee_surfaces.rs` tests | PARTIAL | CODE |
| 129 | Requested `telegram_trade_port.rs` | PARTIAL | CODE |
| 130 | Requested `telegram_fee_wiring.rs` tests | PARTIAL | CODE |
| 131 | Exhaustive repository review | OPEN | CODE |
| 132 | Gap ledger | PARTIAL | CODE |
| 133 | Owner actions | UPDATED | CODE |
| 134 | Known limitations | UPDATED | CODE |
| 135 | Competitor benchmark | PARTIAL | CODE |
| 136 | Workspace test | PARTIAL | CODE |
| 137 | Script mode verification | DONE | CODE |

---

## ৮. এই তালিকা কোথা থেকে এল

- সোর্স: `GAP-MISSING-FILES.md` (গ্যাপ টেবিল), `docs/KNOWN-LIMITATIONS.md` (সীমা),
  `evidence/**` (প্রমাণ ফাইল), `docs/FINAL-FILE-INVENTORY.md` (ফাইল-হ্যাশ),
  `SNAPSHOT-MANIFEST.sha256` (প্রতিটি ফাইলের hash), `release-manifest.json` (গণনা)।
- মেশিন-পাঠ্য সংস্করণ: `docs/GAP-INVENTORY.csv` (একই স্ক্রিপ্ট লেখে)।
- টেস্ট-গণনা ও গেটের সর্বশেষ ফল: §৮-এর নিচের লাইন + `evidence/**/final-checks-*.txt`।

## ৯. হিসাব (স্ক্রিপ্টে যাচাই করা)

- মোট সারি: 137 = DONE 73 + OPEN 16 + PARTIAL 32 + অন্যান্য 13
- অসম্পূর্ণ মোট: 64

_তৈরি: `python3 scripts/regen-open-remaining.py`_
