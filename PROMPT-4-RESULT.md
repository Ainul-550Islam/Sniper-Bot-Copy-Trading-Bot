# PROMPT 4/10 — RESULT: The Tenant Execution Boundary (signing, guard, module wiring, factory)

**Status: COMPLETE.** Every planned surface shipped and verified: the tenant
signing context and transaction-metadata types in `solana-kit`; the final
tenant broadcast guard attached at the executor with fail-closed semantics;
the sniper and copy engines wired for tenant-scoped execution (entries,
exits, sweepers — no internal executor left unguarded); the tenant-local
dedup/counters state; and the server-plane module factory with
repository-backed sinks, proven against a **live PostgreSQL 17.11**
database. All four touched crates compile clean, are `cargo fmt` clean and
`cargo clippy --all-targets -- -D warnings` clean, and the full workspace
`cargo check --workspace --all-targets` passes with **zero warnings**.
Test totals this prompt: **solana-kit 294 passed / 0 failed**, **module-sniper
144 passed / 0 failed**, **module-copy 110 passed / 0 failed**, **sniper-suite
(full suite, every binary) 1288 passed / 0 failed / 12 ignored**, and — the
extended regression sweep over the untouched crates — **bot-core 727 passed /
0 failed against the live database (zero NOT_RUN, PROMPT 3's exact baseline)**,
**module-polymarket 147 / 0**, **module-telegram 21 / 0**, **saas-sdk 32 / 0**.
62 of the passing tests are new tenant-boundary tests.

> **Verification found and fixed five real defects** that compilation alone
> could not catch: a test-helper issuing contexts with two different runtime
> identities; a stamping layer that silently OVERWROTE explicitly attached
> (possibly foreign) tenant metadata — the exact masking hazard the guard
> exists to catch; paper test requests funded by a key the wallet could not
> sign; a persistence sink that recorded unproven broadcasts as `confirmed`;
> and a clippy/`missing_docs` regression on the extended exit path. Details
> in §12.

---

## 1. What was built

PROMPT 4/10 closes the execution-boundary gap PROMPT 3 left open: the data
plane was tenant-scoped, but the ENGINES still signed and broadcast with the
deployment-global hot wallet on deployment-global state. Now:

**A tenant-scoped execution is one unbroken chain:**

```
gateway (authority chain, PROMPT 2/3)
   └─→ TenantExecutionContext            (bot-core, unchanged — ONE identity type)
         └─→ TenantSigningContext        (solana-kit — the adapter the executor sees)
               └─→ TenantTransactionMeta (solana-kit — rides ON the request, NOT in the intent digest)
                     └─→ TenantBroadcastGuard (solana-kit — attached to the Executor;
                          denies BEFORE signing/broadcast/ledger-residue)
                           └─→ Sniper / CopyBot bound via with_tenant_context
                                 (every internal executor — including the exit
                                  sweepers — gets the SAME guard, fail-closed)
                                      └─→ TenantSniperExecutor / TenantCopyExecutor
                                            (authorize → verify_runtime fence →
                                             tenant-local dedup → existing pipeline →
                                             sink-persisted outcomes)
                                            └─→ TenantModuleFactory + RepoExecutionSink /
                                                 RepoCopySink (server plane; every write
                                                 organization-attributed, fail-closed on
                                                 a foreign outcome)
```

* **single-operator deployment is untouched**: a `Sniper`/`CopyBot` built
  WITHOUT a tenant context keeps byte-for-byte the prior behaviour (proven
  by test — §10);
* **the SaaS path fails closed**: wrong organization / runtime / generation /
  module / wallet / signer ⇒ `ExecStatus::Skipped` with a machine-readable
  reason code, no signing, no ledger residue, no sweeper ever runs unguarded;
* **no private keys in tenant config or the database** — the context carries
  a `TenantWalletRef`/`TenantSignerRef` (public references); the key material
  stays where it already lived (§13 for the custody direction);
* **no hardcoded plans or synthetic values** in any production path.

## 2. The five denial layers and where they live

| layer | file | refuses |
|---|---|---|
| context issuance | `bot-core` `TenantExecutionContext::issue` (pre-existing) | incomplete authority, unsound identity |
| executor attach | `solana-kit` `Executor::with_tenant_guard` | guard wallet ≠ executor wallet ("does not match the executor wallet") |
| run() gate | `solana-kit` `Executor::run` / `send_prebuilt` | `missing_tenant_meta`, `organization_mismatch`, `runtime_mismatch`, `generation_mismatch`, `module_mismatch`, `wallet_mismatch`, `signer_revoked` — BEFORE `resolve_intent` and BEFORE the paper early-return, so a deny leaves NO lifecycle residue |
| module authorize | `tenant_executor.rs` (both modules) | the same five identity checks at every scoped entry point, plus the `verify_runtime` fence (rotated runtime ⇒ refuse) |
| sink write | `server` `assert_tenant_scope` | an outcome whose organization ≠ the sink's write scope, BEFORE any repository access |

Every deny label is stable and machine-readable
(`TenantBroadcastDeny::as_str()` / `detail()` now prefixes the label, so log
grep and audit queries work on both forms).

## 3. Complete file inventory

**New (14 files):**

| file | contents |
|---|---|
| `crates/solana-kit/src/tenant_signing_context.rs` | executor-side adapter over the core context; module/mode/wallet/signer accessors; `authorize_wallet` |
| `crates/solana-kit/src/tenant_transaction.rs` | `TenantTransactionMeta` (built `from_context`, deliberately NOT part of `intent_digest`) + `TenantTransaction` outcome wrapper (now exposing `exec_status()` so sinks distinguish CONFIRMED from SENT) |
| `crates/solana-kit/src/tenant_broadcast_guard.rs` | the final guard: seven deny variants, `veto_result()` (Skipped + labelled error), identity accessors, inline tests |
| `crates/solana-kit/tests/tenant_signing_boundary.rs` | 9 integration tests: every deny, the pass-through (paper fills), unguarded executor unchanged, wrong-wallet attach failure |
| `crates/module-sniper/src/tenant_context.rs` | `SniperTenantContext` adapter (module enforcement + tenant-local dedup keys) |
| `crates/module-sniper/src/tenant_state.rs` | FIFO-bounded (8192) tenant dedup + `TenantSniperCounters` |
| `crates/module-sniper/src/tenant_executor.rs` | `TenantSniperExecutor`: authorize / verify_runtime / consider_event (tenant dedup BEFORE pipeline, duplicates return a manual `EntryOutcome` with `RejectReason::DuplicateEvent`) / submit (stamped, guard-verified, sink-persisted — sink errors propagate) / `record_execution` |
| `crates/module-sniper/tests/tenant_execution.rs` | 6 integration tests (foreign metadata denied, bound context passes the gate, authorization chain, unbound sniper unchanged, adapter/dedup/wallet binding) |
| `crates/module-copy/src/tenant_context.rs` | `CopyTenantContext` adapter: module enforcement, **tenant-local leader/event keys** (the same external leader may be tracked by many tenants), paper-default |
| `crates/module-copy/src/tenant_state.rs` | FIFO-bounded tenant event dedup + `TenantCopyCounters` (incl. `guard_denied`) |
| `crates/module-copy/src/tenant_executor.rs` | `TenantCopyExecutor`: authorize / verify_runtime / `process_trade` (tenant dedup → existing pipeline → sink) / `record_outcome`; `TenantCopySink` receives the SOURCE event too (the outcome alone lacks leader/mint/venue attribution) |
| `crates/module-copy/tests/tenant_execution.rs` | 6 integration tests (authorization chain, wrong-wallet construction refusal, tenant-local dedup, two-tenants-same-trade independence, unbound bot unchanged, adapter key scoping) |
| `crates/server/src/tenant/module_factory.rs` | `TenantModuleFactory` (builds both engines over `Arc<Database>`; NEVER issues contexts — that is the gateway's job) + `RepoExecutionSink` (`transactions` rows, signature-idempotent) + `RepoCopySink` (`copy_events` rows, `(organization_id, event_id)`-idempotent) + `assert_tenant_scope` |
| `crates/server/tests/tenant_module_factory_pg.rs` | the real-database suite (gated on `POSTGRES_URL`, `NOT_RUN` otherwise) |

**Modified (10 files):**

| file | change |
|---|---|
| `crates/solana-kit/src/tx.rs` | `TxRequest.tenant: Option<TenantTransactionMeta>` (excluded from `intent_digest`), builders `tenant()/tenant_meta()`; `BuiltTx.tenant` (`from_signed` ⇒ `None`) + `with_tenant()` |
| `crates/solana-kit/src/execute.rs` | `Executor.tenant_guard` + `with_tenant_guard` (attach-time wallet check); gates at the top of `run()` and `send_prebuilt()`; test initializers extended |
| `crates/solana-kit/src/lib.rs` | module declarations |
| `crates/module-sniper/src/lib.rs` | `Sniper.tenant/.tenant_guard`, `with_tenant_context`, `tenant_stamp`/`tenant_stamp_built` (never overwrite an explicit meta), `execute_request`, **sweeper guard propagation (fail-closed)**, module declarations |
| `crates/module-sniper/src/entry.rs`, `exit.rs` | stamping at the broadcast call sites |
| `crates/module-sniper/Cargo.toml` | `async-trait` moved to `[dependencies]` |
| `crates/module-copy/src/lib.rs` | `CopyBot.tenant/.tenant_guard`, `with_tenant_context`, stamp helpers, **`run()` attaches the SAME guard to the exit sweeper or stops the run**, module declarations |
| `crates/module-copy/src/mirror.rs` | stamping of the mirrored buy (request + prebuilt) |
| `crates/module-copy/src/exit.rs` | tenant threading through `sell_position` → `sell_on_curve`/`sell_via_jupiter`; `ExitSweeper.tenant` + `with_tenant_context`; `stamp_copy_request`/`stamp_copy_built` |
| `crates/module-copy/src/event.rs`, `crates/server/src/tenant/mod.rs`, `crates/server/Cargo.toml` | caller updated; `module_factory` declared; `bs58` dev-dependency |
| `release-manifest.json` | `rust_files` count refreshed 506 → 520 (the 14 new sources), enforced by `release_manifest_counts_and_version_are_current` |

## 4. Module wiring — sniper

`Sniper::with_tenant_context(guard)` stores the signing context AND the
guard; the guard is attached to the executor at build time. `Sniper::run`
builds a SEPARATE executor for the exit sweeper — that executor now receives
the SAME guard, and if the attach fails the run loop STOPS (error logged,
error recorded, `set_running(false)`) rather than ever running a
deployment-global sweeper under a tenant. `Sniper::execute_request` stamps
(`tenant: None` ⇒ stamp; an explicit meta ⇒ left untouched for the guard to
judge) and runs. `TenantSniperExecutor` wraps all of it: tenant-local dedup
happens BEFORE the pipeline (a duplicate returns a manual `EntryOutcome` at
stage `Detected` with `RejectReason::DuplicateEvent`), outcomes go to the
sink, sink errors surface to the caller.

## 5. Module wiring — copy

The copy engine has FOUR money-moving surfaces, all covered: the mirrored
buy (request path), the mirrored buy (Jupiter prebuilt path), the whale/rule
exits through `sell_position` (→ `sell_on_curve` / `sell_via_jupiter`), and
the `ExitSweeper`'s own exits on its own executor. The tenant signing
context threads through `sell_position`'s signature (doc header and
`#[allow(clippy::too_many_arguments)]` preserved/restored); the sweeper gains
`with_tenant_context` and `CopyBot::run` attaches the guard fail-closed.
Leader scoping is configuration-driven: `process_trade` hands the TENANT's
own config snapshot to the pipeline, so `sync_leaders` follows the tenant's
wallet list — and `CopyTenantContext::leader_key`/`event_key` keep every
in-memory structure tenant-local even when two tenants track the SAME
external leader address.

## 6. Server plane — factory and sinks

`TenantModuleFactory::new(db)` builds `TenantSniperExecutor` /
`TenantCopyExecutor` from an ALREADY-ISSUED context (issuance stays in the
gateway — the factory adds no authority of its own) and attaches the
repository sinks. `RepoExecutionSink` writes `transactions` rows
(`record_transaction_submitted` is signature-idempotent; a re-record of
another tenant's signature returns `Ok(false)` without revealing whose),
then sets the status with PRECISE landing semantics: only `Confirmed`
becomes `confirmed`; `SendFailed`/`SimulationFailed` (or an error) become
`failed`; a `Sent`/`SendUnknown` broadcast stays `submitted` for the
reconciliation matrix. `RepoCopySink` writes `copy_events` rows (idempotent
on `(organization_id, event_id)` — a replayed feed can never double-count).
Both call `assert_tenant_scope` BEFORE any repository access: a foreign
outcome is refused without touching the database. Write scopes are attributed
(`tenant-module-factory`, `WriteOrigin::Job`).

## 7. Fail-closed disciplines (the invariants this prompt enforces)

1. **No unguarded internal executor**: every executor a module constructs
   (sniper sweeper, copy exit sweeper) gets the SAME guard or the run stops.
2. **Stamp, never rewrite**: an explicitly attached tenant meta is judged by
   the guard, never silently replaced — a rewrite would mask exactly the
   cross-tenant mistake the guard exists to catch.
3. **Deny before residue**: the run() gate sits before `resolve_intent` and
   before the paper early-return, so a veto leaves no ledger/lifecycle state.
4. **Tenant-local dedup before the pipeline**: one tenant's dedup set can
   never suppress another tenant's trade — the same external event is new
   for every tenant exactly once.
5. **Sink failures surface**: persistence errors propagate to the caller;
   they are never swallowed, and they never "un-broadcast" a transaction
   that already left the process.
6. **Operator mode is the default**: no tenant context ⇒ the exact prior
   behaviour, verified by dedicated tests in all three engines.

## 8. What deliberately did NOT change

* `intent_digest` — tenant metadata rides beside the intent, never in it;
  execution intents stay module-scoped and unchanged.
* The gateway/authority chain, entitlements, risk guards (PROMPT 2/3
  surfaces) — the boundary consumes their output, it does not re-implement
  them.
* The legacy deployment API and single-operator `main.rs` spawn path.
* Key custody: `SignerProvider::Local` remains the only implemented backend.
  No fake Vault/KMS/HSM was added (§13 records the direction instead).

## 9. Phase-0 research → implementation decisions

| finding | decision it drove |
|---|---|
| Solana production signing guidance ("where does the private key live, and who may authorize a signature with it") — official `solana.com` "Signing in Production" | the guard authorizes at the EXECUTOR boundary (attach-time wallet check + per-broadcast gate), not inside the signing primitive; `TenantSignerRef` is a public reference, never key material |
| a Solana transaction requires a valid signature from EVERY required signer (anza offline-signing docs) | test/production requests fund transfers from the fee payer; the build fails loudly on any signer the wallet cannot satisfy (`SignerMismatch` surfaced, never bypassed) |
| hot-wallet guidance: separate active wallets per purpose | per-tenant wallet binding — each tenant's executions fund from the wallet bound in THEIR context; the guard refuses any other funding wallet |
| Polymarket async commit pipeline (Jul 24 2026: FAK/FOK matches return `tradeIDs`, hashes resolved by polling) + clob-client-v2 v1.2.0 (positionID orders, Exchange V3 signing) | recorded for the polymarket module's next prompt; nothing in this prompt's Solana-side boundary contradicts it (no polymarket execution path was touched) |

## 10. Verification evidence (commands and numbers)

Environment: rustc **1.98.1** (pinned), PostgreSQL **17.11**, `--test-threads=1`.

| command | result |
|---|---|
| `cargo test -p solana-kit` | **294 passed, 0 failed** (7 test binaries; includes the 9-test `tenant_signing_boundary` integration suite and 17 tenant lib tests) |
| `cargo test -p module-sniper` | **144 passed, 0 failed, 1 ignored** (11 test binaries; includes the 6-test `tenant_execution` suite) |
| `cargo test -p module-copy` | **110 passed, 0 failed** (14 test binaries; includes the 6-test `tenant_execution` suite) |
| `cargo test -p sniper-suite --lib` | **460 passed, 0 failed, 3 ignored** (includes the factory scope-check test) |
| `POSTGRES_URL=… cargo test -p sniper-suite --test tenant_module_factory_pg` | **2 passed, 0 failed against the real database** (and `NOT_RUN` + green without the env, exactly like the PROMPT 3 gates) |
| `POSTGRES_URL=… cargo test -p sniper-suite` (every test binary) | **1288 passed, 0 failed, 12 ignored** — includes the tenant suite, the lib, and all pre-existing integration suites (backup/restore, billing, SaaS, observability, package readiness) |
| `POSTGRES_URL=… cargo test -p bot-core` | **727 passed, 0 failed, 1 ignored, ZERO `NOT_RUN`** — PROMPT 3's live-database baseline reproduced exactly, i.e. no regression in the tenant data layer |
| `cargo test -p module-polymarket` / `-p module-telegram` / `-p saas-sdk` | **147 / 21 / 32 passed, 0 failed** — untouched crates unaffected |

Workspace-wide: **2 763 tests passed, 0 failed** across all eight crates on
this exact tree (14 ignored: pre-existing network/Redis-gated and opt-in
fixtures, unchanged by this prompt).
| `cargo check --workspace --all-targets` | **0 errors, 0 warnings** |
| `cargo fmt --all --check` | clean |
| `cargo clippy -p solana-kit -p module-sniper -p module-copy -p sniper-suite --all-targets -- -D warnings` | clean |

**62 new tests** across the six new/extended suites (9 + 17 + 6 + 12 + 6 + 9
+ 1 + 2), every deny path asserted on the machine-readable label.

## 11. Cross-tenant isolation matrix (what the new tests prove)

| attack / hazard | test | outcome |
|---|---|---|
| tenant B's metadata on tenant A's executor | `a_tenant_sniper_cannot_submit_with_a_foreign_contexts_metadata`; guard tests | `Skipped` + `organization_mismatch`, no broadcast |
| stale generation (rotated runtime) | guard + sniper + copy suites | `Skipped` + `generation_mismatch` |
| wrong module meta (sniper meta on copy and vice versa) | guard suite + `a_sniper_context_cannot_drive_a_copy_executor` | `module_mismatch` / `wrong_module` |
| request with NO tenant metadata on a guarded executor | `missing_metadata_is_denied_before_broadcast` | `Skipped` + `missing_tenant_meta` |
| guard built for another wallet attached to an executor | `attaching_a_guard_for_another_wallet_fails_at_construction` | attach fails; executor unusable, never unguarded-and-running |
| executor funded by a wallet other than the context's | `a_wallet_bound_to_another_tenant_cannot_drive_the_executor` (both modules) | construction refused (`wallet_mismatch`) |
| explicitly attached (foreign) meta silently restamped to own identity | fixed hazard, guarded by the never-overwrite rule + the foreign-metadata test | overwrite can no longer happen |
| one tenant's dedup suppressing another's trade | `two_tenants_seeing_the_same_leader_trade_both_process_it`; sniper cross-context test | both tenants process the same external event independently |
| replayed feed double-counting | `the_same_leader_trade_is_deduplicated_tenant_locally` + PG replay test | in-process dedup + `(organization_id, event_id)` upsert ⇒ single row |
| sink persisting another tenant's outcome | PG suite (both sinks) | `organization mismatch` refused BEFORE any database access |
| tenant A's transaction visible to tenant B | PG suite | `TenantExecutionRead::transaction` returns `None` under B's scope |
| unproven broadcast recorded as landed | fixed defect; PG suite asserts `submitted` for a `Sent` result | status mapping is precise |
| operator mode regressing | `an_unbound_sniper…` / `an_unbound_copy_bot…` / `unguarded_executor…` | no tenant context ⇒ prior behaviour (executes in paper, no skips) |

## 12. Real defects found and fixed by verification

1. **Test-helper identity bug** (`module-sniper/src/tenant_context.rs`): the
   helper minted the authority checklist for ONE runtime id and issued the
   context for a DIFFERENT one — the fingerprint never matched and all five
   tests panicked at the issuance unwrap. Fixed: one runtime identity for
   both. (Production code was correct; the helper now documents the trap.)
2. **Stamping overwrite hazard** (`Sniper::tenant_stamp` /
   `tenant_stamp_built`, and the copy equivalents): an explicitly attached
   (possibly foreign) meta was silently REWRITTEN to the engine's own
   identity — masking exactly the cross-tenant mistake the guard exists to
   catch, and it made the foreign-metadata test fail in a way that revealed
   the bug. Fixed: explicit metadata is judged, never rewritten.
3. **Unsignable test requests**: paper transfers funded from a random key
   while the message's fee payer was the wallet — the documented
   every-required-signer rule made the build fail (`SignerMismatch`), which
   surfaced as confusing executor errors. Fixed: fund from the paying
   wallet.
4. **Landing-status overstatement** (`RepoExecutionSink`): `succeeded()`
   counts `Sent` (broadcast, landing unproven) as success, so the first
   mapping recorded pending broadcasts as `confirmed`. Fixed: the wrapper
   exposes `exec_status()` and the sink maps only `Confirmed` → `confirmed`.
5. **Exit-path lint regression**: threading `tenant` through
   `sell_position` lost the function's doc header and tripped
   `too_many_arguments` (13/7) under `-D warnings`; restored the header and
   the explicit allow.
6. **Stale release manifest** (found by the full server-suite run):
   `release_manifest_counts_and_version_are_current` failed because the
   tree's `.rs` count had grown 506 → 520 — exactly the 14 files this
   prompt adds — while `release-manifest.json` still recorded 506.
   Refreshed the count (docs 101 and migrations 34 were already current);
   the full suite then passed 1288/1288 with no other failure.

## 13. Post-implementation forensic re-search

Re-verified after implementation against current external sources:

* **`solana.com/docs/core/transactions/signing-in-production`** — the
  production question is "where does the private key live, and who is
  allowed to authorize a signature with it". Our boundary answers it at the
  executor: attach-time wallet verification plus a per-broadcast guard, with
  the tenant holding only public references (`TenantWalletRef`,
  `TenantSignerRef`). The page's custody ladder (local keypair → KMS/HSM →
  MPC) matches the repo's stance: `SignerProvider` models the providers; no
  fake backend was implemented, and the Solana Foundation "Keychain" unified
  signing library is the natural integration point when a real KMS/HSM
  backend is built — noted as future work, NOT bolted on here.
* **anza `docs.anza.xyz` offline-signing semantics** — a transaction
  executes only with valid signatures from every required signer; our
  builder's `SignerMismatch` failure (surfaced during this prompt's tests)
  is the correct, non-bypassable behaviour and the tests now fund from the
  fee payer accordingly.
* **2026 wallet security guidance** (hot/cold separation, per-purpose
  wallets) — consistent with per-tenant wallet binding; nothing to change.
* **Polymarket changelog / clob-client-v2 v1.2.0** (async commit pipeline,
  `tradeIDs`, positionID + Exchange V3 signing) — re-checked for drift
  against this prompt's scope: no polymarket execution path was modified, so
  no conflict; the findings remain queued for the polymarket prompt.

No contradiction between the implementation and any current external
guidance was found.

## 14. Deviations & judgment calls

1. **The factory depends on `Arc<Database>`, not the
   `TenantTradingDataPlane` type.** The plane's HTTP handlers live in the
   binary's module tree; a lib-visible factory depending on it would have
   dragged the whole API surface into the library crate. The factory builds
   its write scopes directly (`WriteOrigin::Job`, actor
   `tenant-module-factory`) over the same `bot-core` repositories — the
   plane remains the read surface.
2. **`TenantCopySink::record_outcome` receives the SOURCE event as well as
   the outcome** — `CopyOutcome` alone lacks the leader/mint/venue/amount
   attribution the `copy_events` rows require. The sniper sink needs no
   equivalent (`TenantTransaction` already carries what `transactions`
   needs).
3. **`TenantTransaction::exec_status()` added** (a read accessor on the
   wrapper) rather than widening `succeeded()` — gate semantics
   (Sent = not failed) and persistence semantics (Sent ≠ landed) are
   deliberately different and now both explicit.
4. **`TenantSniperExecutor::record_execution` /
   `TenantCopyExecutor::record_outcome` public methods** — reconciliation
   paths need to persist late-arriving outcomes through the SAME sink the
   factory attached; the PG suite exercises exactly that object.
5. **Temporarily-added `pub mod trading_data_plane` in the server lib was
   reverted** (net zero change) once the factory was decoupled — the lib
   crate keeps its original surface.

## 15. No-omission declaration

Every file written or modified in this prompt is complete in the repository
— no placeholders, no `TODO`/`FIXME`/`stub` markers, no `// existing code`
or `/* omitted */` elisions, no "for brevity" comments, no diff/patch
fragments. All pre-existing behaviour is preserved and covered by the green
suites listed in §10 (single-operator deployment paths run unmodified in the
unbound tests; the legacy API compiles untouched; bot-core's live-database
suites reproduce PROMPT 3's 727-passed baseline). Nothing was deleted,
commented out, or skipped to make a check pass; the ignored tests are the
pre-existing network/Redis-gated and opt-in-fixture ones (14 across the
workspace), unchanged by this prompt.

## 16. How to reproduce

```bash
# toolchain is pinned (1.98.1) via rust-toolchain.toml
cargo fmt --all --check
cargo clippy -p solana-kit -p module-sniper -p module-copy -p sniper-suite \
    --all-targets -- -D warnings
cargo test -p solana-kit   -- --test-threads=1   # 294 passed
cargo test -p module-sniper -- --test-threads=1  # 144 passed, 1 ignored (network-gated)
cargo test -p module-copy  -- --test-threads=1   # 110 passed
cargo test -p sniper-suite --lib -- --test-threads=1  # 460 passed

# the real-database suites (NOT_RUN + green without the variable):
export POSTGRES_URL="postgres://user:pass@host:5432/db"   # migrations auto-apply
cargo test -p sniper-suite --test tenant_module_factory_pg -- --test-threads=1
cargo test -p bot-core     -- --test-threads=1   # 727 passed, zero NOT_RUN
cargo test -p sniper-suite -- --test-threads=1   # 1288 passed (every binary)
```
