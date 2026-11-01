# FINAL COMPLETED TREE

Output section B of the master prompt: every entry that differs from `HEAD 4a59411`, with its
marker and a factual description of what the file does after this batch. Generated from
`git diff --name-status HEAD` — nothing invented, nothing omitted.

**Entries:** 74 (deleted files included)

```
[DEL] .cargo/env
      #     # removed: machine-local/leftover file that must not ship (env dump, stale checksum file, temp test log)
[DEL] .config/solana/install/config.yml
      #     # removed: machine-local/leftover file that must not ship (env dump, stale checksum file, temp test log)
[DEL] SHA256SUMS
      #     # removed: machine-local/leftover file that must not ship (env dump, stale checksum file, temp test log)
[DEL] run-part2.sh
      #     # removed: machine-local/leftover file that must not ship (env dump, stale checksum file, temp test log)
[DEL] run-tests.out
      #     # removed: machine-local/leftover file that must not ship (env dump, stale checksum file, temp test log)
[MOD] CHANGELOG.md
      # changelog entry for the copy-follow wiring (engine → live feed → staged pipeline) and its honest limits
[MOD] Cargo.lock
      # dependency graph after `cargo update` (166 transitive bumps; RustSec advisories 18 → 9, the rest inherited from the Solana SDK 2.3.13 tree and not silenced)
[MOD] DELETE-LIST.txt
      #     # audit trail of the junk files removed by this batch
[MOD] README.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] SNAPSHOT-MANIFEST.sha256
      #     # regenerated canonical checksum manifest over the tracked tree
[MOD] apps/control-plane/e2e/exports-and-feeds.spec.ts
      #     # e2e: row locator scoped to the row's own accessible name — removes the flaky strict-mode match (68/1 → 69/0)
[MOD] config.toml
      # `[polymarket]` copy-follow keys added to the shipped working config (all default-off: empty wallet list)
[MOD] config.toml.example
      # the same keys documented in the example config, so an operator sees every copy bound; asserted by `bundled_example_config_parses`
[MOD] crates/core/src/config.rs
      # `PolymarketConfig` gains `copy_followed_wallets` + `copy_poll_interval_secs` + the four sizing/risk bounds (struct, `Default` impl, serde defaults, and the documented-keys guard test)
[MOD] crates/module-polymarket/src/copy.rs
      # `CopyEngine::seen` read-only peek so a replayed page row costs no book fetch (the authoritative dedup stays inside `evaluate`)
[MOD] crates/module-polymarket/src/discover.rs
      # `quotes_for` promoted to `pub(crate)` so copy entries are priced through exactly the same websocket-then-REST path as strategy entries
[MOD] crates/module-polymarket/src/lib.rs
      # run loop gains the copy-follow engine/feed construction and the `Tick::Copy` arm (gated on module enabled + kill switch released)
[MOD] crates/module-polymarket/tests/common/mod.rs
      # mock harness gains a `/data-api/trades` route, scriptable leader rows, an HTTP-500 mode and request inspection — test-only
[MOD] crates/server/src/main.rs
      #     # boot sequence: resolve the deployment organization id through `public.deployment_organization_id()` before projecting it (fixes the `organizations_slug_key` startup crash); spawn the DCA schedule worker with app+system pools; replace the no-op lifecycle worker with a real drain loop
[MOD] crates/server/src/provisioning/lifecycle_worker.rs
      #     # durable tenant-lifecycle driver: `drain_due_jobs` claims due `tenant_lifecycle_jobs` rows with `FOR UPDATE SKIP LOCKED` under an audited system scope, advances one phase per tick, parks at `waiting` where no unattended side effect exists
[MOD] crates/server/src/saas/mod.rs
      #     # `ensure_deployment_organization_with_id` adopts the schema's deployment-org id instead of minting a second one; `ensure_deployment_organization` delegates with no hint
[MOD] crates/server/src/saas/readiness.rs
      #     # operator readiness now reports observed values: real migration high-water from `_sqlx_migrations`, worker status mirroring the spawn predicate, providers as `unprobed` instead of `ready`
[MOD] crates/server/src/trading_data_plane/mod.rs
      # registers the new `limit_order_ports` module so the limit-order worker has a live mark source and an idempotent intent sink
[MOD] docs/ARCHITECTURE-OVERVIEW.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] docs/CAPABILITY-MATRIX.md
      # stat marker synchronised (`test_attrs_plain`), a regenerated count — never a pass/fail claim
[MOD] docs/DELIVERY-MANIFEST.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md
      # stat marker synchronised (`rust_files`) after the tree changed
[MOD] docs/EVIDENCE-INDEX.md
      # stat marker synchronised (`test_attrs_plain`)
[MOD] docs/FINAL-RELEASE-AUDIT.md
      # RETIRED banner: points readers at the current audit and says the old numbers describe the 2026-09-19 tree, preserved unchanged
[MOD] docs/IP-OWNERSHIP-REGISTER.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] docs/OPERATIONS-RUNBOOK.md
      # stat marker synchronised (`rust_files`)
[MOD] docs/PENETRATION-TEST-READINESS.md
      # stat markers synchronised (`rust_files`, `test_attrs_plain`) in the tester artifact list
[MOD] docs/POLYMARKET-ENGINE.md
      # new §12a copy-follow (files, config, per-poll sequence, skip vocabulary, honesty limits), config rows, component map and test inventory
[MOD] docs/PRODUCTION-READINESS-MATRIX.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] docs/RELEASE-NOTES-0.1.0.md
      # stat marker synchronised (`test_attrs_plain`)
[MOD] docs/SECURITY-CONTROLS-MATRIX.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] docs/SECURITY-THREAT-MODEL.md
      # stat marker synchronised (`test_attrs_plain`) in the verification footer
[MOD] docs/SELLER-FACT-SHEET.md
      # stat marker synchronised (`test_attrs_plain`) in the test-evidence section
[MOD] docs/SELLING-LISTING-SOURCE.md
      # stat marker synchronised (`test_attrs_plain`); no marketing claim added
[MOD] docs/STATS.md
      #     # regenerated by scripts/generate-stats.sh — the only source of repository numbers
[MOD] docs/TECHNICAL-FACT-SHEET.md
      # stat marker synchronised (`test_attrs_plain`)
[MOD] docs/TRANSACTION-READINESS-REPORT.md
      #     # stat markers synchronised to the regenerated numbers
[MOD] release-manifest.json
      #     # regenerated product/manifest counters (files, rust files, docs, migrations high water)
[MOD] scripts/apply-batch01-deletes.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/bootstrap-config.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-batch02-ownership.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-batch03-ownership.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-batch04-ownership.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-batch07-ownership.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-cleanup-ownership.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/check-rls-coverage.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/cleanup-junk-v2.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/cleanup-tracked-junk.sh
      #     # dry-run-by-default removal of verified machine-local junk
[MOD] scripts/fix-script-modes.sh
      #     # restores the executable bit on tracked shell scripts
[MOD] scripts/regen-snapshot-manifest.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/run-dataset-capture.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/telegram-trade-smoke.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/test-with-services.sh
      #     # service-test runner: `--existing` mode, health checks, migration run, nonzero-passed enforcement, cleanup trap
[MOD] scripts/untrack-batch01-junk.sh
      #     # untracks the Batch 01 junk set
[MOD] scripts/verify-batch-complete.sh
      #     # historical batch gate (fails on main today; see OWNER actions)
[MOD] scripts/verify-batch07-complete.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] scripts/verify-raydium-idl.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] tests/release/evidence_truth.sh
      #     # executable bit corrected to 100755 — content byte-identical
[MOD] tests/release/forensic_sql_extractor.sh
      #     # executable bit corrected to 100755 — content byte-identical
[NEW] crates/module-polymarket/src/copy_feed.rs
      # NEW: leader-trade feed over the public data API's `/trades` (pure, total parser; failures are errors, never "no trades"; liquidity/tick size never invented)
[NEW] crates/module-polymarket/src/copy_run.rs
      # NEW: `PolyBot::copy_poll_once` — the copy engine's production caller (poll → resolve market → quote → evaluate → `process_signal`), plus `copy_engine_from_config` and `CopyPollReport`
[NEW] crates/module-polymarket/tests/copy_follow.rs
      # NEW: 8 integration tests driving leader trades through the real staged pipeline (mirror → paper fill, dedup, LEADER_SELL, NOT_FOLLOWED_WALLET, SLIPPAGE_EXCEEDED, feed error, no-wallet, malformed rows)
[NEW] crates/server/src/trading_data_plane/limit_order_ports.rs
      #     # production ports for the limit-order worker: live Jupiter mark source (`usdPrice(mint)/usdPrice(WSOL)`) and an idempotent `execution_intents` hand-off — no invented price, no claimed fill
[NEW] docs/FINAL-CODE-COMPLETENESS-AUDIT.md
      #     # requirement→file matrix in the mandated columns for all 30 Batch-01 paths and every Phase-2 area A–J
[NEW] docs/FINAL-COMPLETED-TREE.md
      #     # output section B: every changed entry with its [NEW]/[MOD]/[DEL] marker and a factual purpose comment
[NEW] docs/FINAL-FILE-INVENTORY.md
      #     # generated inventory: every shipped file with path, type, size, SHA-256, NEW/MOD/original, inspected and tested
[NEW] docs/FINAL-RELEASE-AUDIT-2026-10-31.md
      #     # closing audit for this pass: changes, requirements, defects, verification, honest percentages, owner actions
[NEW] docs/FINAL-TEST-RESULTS.md
      #     # executed commands, real counts, and every NOT RUN / BLOCKED row
[NEW] docs/PROMPT-COMPLIANCE-AUDIT.md
      #     # section-by-section compliance of this delivery against the 935-line master prompt, including what was not done
```

## Notes

- Every `[NEW]` document is inside the ZIP under `docs/`.
- The three NEW Polymarket files are one change: `copy_feed.rs` (source), `copy_run.rs`
  (caller) and `tests/copy_follow.rs` (proof). Without all three the copy engine would have
  remained reachable only from its own unit tests.
- Nothing was committed or pushed: this tree is the working tree at delivery time.
