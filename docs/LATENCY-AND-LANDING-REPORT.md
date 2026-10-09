# LATENCY-AND-LANDING-REPORT.md

**Status: NO MEASUREMENTS EXIST YET.** Every cell in the results tables below
is `NOT_RUN`. This document defines the method, the metric vocabulary, and the
publication rule so that the first real run produces a report a buyer can
audit — but per project rule 1 (never fabricate data) and rule 2 (a gap
closes only with `evidence/live/*.json` "PASSED" or a passing test), **no
latency figure may be quoted anywhere in this repo until the corresponding
evidence file is PASSED.**

Evidence anchor: `evidence/live/latency_report.json` — currently `NOT_RUN`.

---

## 1. What "latency" means here (stage definitions)

A launch-snipe has four measurable stages. Each is timed with monotonic
clocks inside the process that owns the stage boundary, so no cross-machine
clock skew enters a single stage's number.

| Stage | Start | End |
|---|---|---|
| **detect** | feed event timestamp (Geyser `transactionSubscribe` / PumpPortal WS / logs-poll receipt) | the moment the decision pipeline accepts the launch (post-gates) |
| **build+sign** | decision accepted | signed `VersionedTransaction` ready |
| **submit** | first send call issued | send RPC/Jito accepted the transaction (ACK, not landing) |
| **landed** | submit ACK | transaction observed in a confirmed slot (signature status query or subscription) |

Derived metrics:
- `detect_to_submit` = detect + build+sign + submit (our controllable path).
- `detect_to_landed` = the end-to-end number a buyer actually cares about.
- `landing_rate` = landed / submitted over the run window.

## 2. Statistical contract

- Report **p50 and p95** (and n) for every stage; never a bare average.
- Minimum sample: **n ≥ 100 events per provider/region cell** before any
  number is published; smaller samples are labelled `n=<k>, provisional`.
- Every run records: RPC/send provider(s), region/endpoint of THIS machine,
  cluster (mainnet/devnet), commitment level, priority-fee policy, Jito tip
  policy, and the broadcast mode (`Rpc` / `Jito` / `JitoThenRpc` / `Race`).
- Raw per-event data is appended to the run's JSON so a buyer can recompute
  percentiles; the summary must be derivable from the raw rows.

## 3. Measurement harness (already implemented, not yet executed live)

The harness lives at `crates/solana-kit/tests/latency_bench.rs` and has two
halves:

1. **Offline, always-run** (CI-safe): warm-account-cache behaviour with the
   network dead, and broadcast fan-out selection against a mock JSON-RPC
   server. These prove the machinery, not any speed.
2. **Gated benchmarks** — environment-gated, never run unattended:
   - `E2E_NETWORK=1`: p50/p95 round-trip latency of the hot-path RPC calls
     (`getSlot`, `getLatestBlockhash`, `simulateTransaction`);
   - `E2E_LIVE=1` (implies a funded wallet): landing rate through the real
     executor, sequential vs fan-out.
   Gated tests print a percentile report and only assert success/landing —
   they deliberately never assert wall-clock bounds, so slow networks cannot
   flake CI.

Command for a full live capture:

```bash
E2E_NETWORK=1 E2E_LIVE=1 RPC_URL=<funded-endpoint> \
  cargo test -p solana-kit --test latency_bench
```

Detect-stage timing additionally requires the feed harness under
`module-sniper` against a real launch stream; those per-feed timestamps feed
the same report schema.

## 4. Region × provider matrix (template — ALL CELLS NOT_RUN)

| Region (measurement host) | Provider | detect p50/p95 | submit p50/p95 | detect→landed p50/p95 | landing rate | Evidence |
|---|---|---|---|---|---|---|
| (TBD) | Direct RPC (Helius/Triton class) | NOT_RUN | NOT_RUN | NOT_RUN | NOT_RUN | `evidence/live/latency_report.json` |
| (TBD) | Jito bundles | NOT_RUN | NOT_RUN | NOT_RUN | NOT_RUN | same anchor, separate run id |
| (TBD) | Race mode (Jito + staked RPC parallel) | NOT_RUN | NOT_RUN | NOT_RUN | NOT_RUN | same anchor, separate run id |

Rows are added by real runs only. "(TBD)" regions are filled in when the
measurement host is chosen; publishing a number with an unstated host region
is prohibited (region dominates network RTT).

## 5. What is known WITHOUT measurement (facts, not latency claims)

- The offline latency tests pass in CI: cache-warm reads and fan-out
  selection behave as designed (test-covered, deterministic).
- Broadcast `Race` mode and dynamic tip-floor selection are implemented
  (`solana-kit/src/execute.rs`, static review); their live advantage is
  UNMEASURED.
- Competitor latency claims (e.g. vendor-published sub-400 ms figures) are
  vendor marketing; see `docs/COMPETITOR-BENCHMARK.md` §5 — we neither
  confirm nor beat them until §4 has PASSED rows.

## 6. Publication rule

A latency figure may appear in README, marketing docs, or buyer material
ONLY if:
1. the row's evidence run is `PASSED` in `evidence/live/latency_report.json`
   with a non-empty attestation (run id + raw-data location),
2. n ≥ 100 for that cell, and
3. the claims gate (`scripts/verify-marketing-claims.sh`) passes with the
   figure present.

Anything else is a gap, not a fact.
