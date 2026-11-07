# Buyer risk register — historical handover baseline

This is a historical handover register for sniper-suite 0.1.0, not a
current security certification. Counts and freeze evidence in the table are
historical snapshots, not results of the latest source review. Subsequent
manual review has reproduced software defects; passing tests, operational
controls and external prerequisites do not establish that no defects remain.
The full manual audit is still incomplete.

Fields: **Risk** → **Evidence** → **Mitigation recorded at handover** →
**Operational consequence** → **Buyer action**. Infrastructure and legal
responsibilities are distinguished from code responsibilities, not used to
exclude code defects or unreviewed paths.

| # | Risk | Evidence | Current mitigation (delivered) | Operational consequence | Buyer action |
|---|------|----------|-------------------------------|------------------------|--------------|
| 1 | **No external security audit** — any component; especially the on-chain staking program | Root `SECURITY.md`; `docs/SECURITY.md`; `release-manifest.json` → `not_executed_environment_blocked` | 537+71/3 test matrix (audit pass; 521+48/2 at freeze) incl. tamper/concurrency/dedup/recovery suites; clippy `-D warnings`; audit/deny gates; docs block staking mainnet deployment until an audit passes; paper default + dual live gates limit blast radius | An undiscovered vulnerability could be exploited: staking vault funds at risk on-chain; trading-path edge cases could misbehave under adversarial conditions | Commission an independent audit before staking mainnet deployment; consider a trading-path review before funded live operation |
| 2 | **Funded live execution never generally tested** (no funded keypair existed; requires explicit approval) | `release-manifest.json`; `docs/TESTING.md` §Known gaps; `docs/archive/AUDIT.md` | `simulate` mode RPC-simulates real transactions; paper mode vs live data; reconcile-before-restart; handoff grace; kill switch; daily-loss auto-disable | Real-money behavior (slippage, landing rate, partial fills, venue edge cases) unproven at scale; first funded runs carry discovery risk | Gradual funded validation under operator supervision, smallest sizes first (`docs/BUYER-DEPLOYMENT.md` §15, `docs/DEMO-RUNBOOK.md`) |
| 3 | **Staking deployment identity not finalized** — `declare_id!` is a pre-deploy placeholder; program deployed nowhere | `programs/staking-suite/src/lib.rs`; `docs/STAKING.md`; `docs/HANDOVER.md` §5.2 | Two-step admin transfer prevents key-typo lock-in; parameter timelock with permissionless apply; hard caps; full deploy+genesis sequence documented; genesis mint latched one-shot | Module 4 cannot run live until deployed; deploying under a lost/mismatched keypair would strand governance or require id change + rebuild | Deploy under the placeholder id with the matching keypair (or change id+keypair and rebuild); initialize with multisig admin (Squads/Realms); timelock ≥ 24h; plan the one-shot GenesisMint |
| 4 | **Production RPC dependency & provider rate limits** | `crates/solana-kit/src/rpc.rs`; `docs/OPERATIONS.md` degradation matrix | Retry/failover chokepoint with consecutive-failure tracking; optional broadcast fan-out; visible degradation (readiness 503, `bot_rpc_*` metrics); intents reconciled, never blindly re-broadcast | Provider outage/throttling ⇒ slower detection, failed broadcasts, delayed reconciliation; degraded; money-moving safety still depends on the actual fencing, intent-journal and reconciliation paths | Contract ≥ 1 quality provider (2 enables fan-out/failover); alert on `bot_rpc_requests_total{outcome="fatal"|"exhausted"}` |
| 5 | **Geyser / feed-provider dependency** (Yellowstone-compatible Geyser, PumpPortal) | `crates/solana-kit/src/{events,pumpportal}.rs`; `docs/THIRD-PARTY.md` §3 | Both feeds are optional: poll fallback preserves functionality; mock-tested resubscribe/reconnect; failed-tx skipping | Without a provider: higher detection latency (polling); provider outage: feed gaps until reconnect/fallback | Decide provider strategy (Geyser and/or PumpPortal vs polling); monitor `bot_ws_*` counters; contract providers in the buyer's name |
| 6 | **Third-party API/protocol changes** (pump.fun, PumpSwap, Raydium, Jupiter, Polymarket, Telegram) | `docs/IP-COMPONENTS.md`; `docs/BUYER-RISK-REGISTER.md` history; mock harnesses in `tests/` | Per-venue builders isolated; strict deserialization fails loudly rather than trading on garbage; CI + release-check catch regressions on upgrade | A venue change can break entries/exits/orders until the integration is updated; failure mode is loud (errors/rejections), not silent | Watch venue changelogs; re-run the gate after dependency bumps; budget maintenance for venue drift |
| 7 | **Docker path not executed in delivery sandbox** (no daemon) | `release-manifest.json`; `.github/workflows/ci.yml` docker job | Multi-stage non-root healthchecked image, pinned base; `docker compose config` syntax gate; static inspection done | Image/compose defects would surface only at the buyer's first build | Run `docker compose up --build` + health smoke early in acceptance (`docs/BUYER-QUICKSTART.md` §9) |
| 8 | **CI never run on a real runner** (workflow delivered, unexecuted from delivery environment) | `.github/workflows/ci.yml`; `docs/HANDOVER.md` §3 | Every CI step has a local equivalent that passed in the freeze gate (release-check 20/20 mirrors the workflow); toolchain + service images pinned | Runner-image or service-container quirks possible on first run | Trigger CI on the buyer's remote; fix runner-specific issues before relying on it as a gate |
| 9 | **Secret infrastructure is buyer-side** (env/secret-store quality, key custody) | `docs/SECURITY.md`; `docs/SUPPORT-HANDOVER.md` §6 | Env-var indirection supported; secret-scan release gate; signer boundary (modules never see keys); token-redaction regression; non-loopback bind refused without auth | Weak custody ⇒ key compromise ⇒ direct fund loss; inline configuration, Debug/serialization and logging paths still require review; deployment controls cannot protect a leaked environment | Real secret store; rotate every credential at transfer; restrict prod-env readers; consider implementing the `vault`/`kms` signer extension points |
| 10 | **Production infrastructure is buyer-provided** (PostgreSQL ≥ 16, Redis 7, hosting, monitoring) | `docs/SCOPE-BOUNDARY.md` §2; `docs/DEPLOYMENT.md` | PG is the single durable truth with forward-only migrations; Redis non-authoritative by design; probes + metrics ready to wire; backup/restore procedures and tooling present; real restore success requires a drill | Undersized/unmonitored PG or hosting ⇒ availability and recovery risk; Redis loss is tolerable by design, PG loss is not | Size and monitor PG (with backups per `docs/BACKUP-RESTORE.md`); run the restore drill once on real infra |
| 11 | **Regulatory / legal responsibility** (crypto trading, prediction markets, token issuance) | README §Disclaimer; `docs/SCOPE-BOUNDARY.md` §4 | Paper default; dual live gates; no automated legal decisions anywhere | Operating in a restricted jurisdiction or against venue ToS ⇒ legal exposure independent of software quality | Jurisdiction-appropriate legal review before funded operation or token distribution; verify Polymarket access legality |
| 12 | **Multi-replica behavior tested at 2 replicas only** | `distributed_integration` 4/4, `two_replica_mirror` 1/1; `docs/DISTRIBUTED.md` | Backend-atomic claims (PG authoritative); fencing tokens; `execution_claim_events` lineage for forensics; tighten-only risk oracle | Unforeseen coordination edge cases possible at larger N (invariant mechanisms are N-general, but untested beyond 2) | Stay ≤ 2 active replicas until larger topologies are exercised in buyer staging |
| 13 | **Ownership/legal metadata placeholders** (LICENSE holder, repo URL, security contact) | `docs/HANDOVER.md` §5; `LICENSE`; root `SECURITY.md` | Placeholders are deliberate and documented; no fake values ship; version identity gated | Ambiguous copyright attribution; misrouted vulnerability reports | Complete the three fill-ins at transfer (legal action, not code) |
| 14 | **Point-in-time supply-chain scans** (audit/deny passed at freeze, 2026-09-18) | `docs/THIRD-PARTY.md` §5; `release-manifest.json` | Both lockfiles committed; audit/deny are release + CI hard gates going forward | Advisories published after the freeze are not reflected until re-run | Re-run `cargo audit` / `cargo deny check` periodically and on every dependency change |

## Design goals and scope boundaries — not absence-of-risk guarantees

- **Redis failure:** Redis is not the authoritative financial ledger, but
  losing a claim hash can affect coordination and fencing. Successful lease
  renewal must preserve the hash lifetime; handoff retention must cover its
  grace interval. Intent journaling and reconciliation remain necessary, not
  proof that every Redis-loss or epoch-reuse schedule is safe.
- **Process crash mid-execution:** intent journaling and startup reconciliation
  are intended backstops. Passing exercised crash paths does not prove every
  concurrency, cancellation or recovery schedule safe.
- **Audit integrity:** APIs are intended to expose append-only operations and
  hash-chain verification. Write atomicity, missing best-effort events and
  authorization paths still require review; this is not an impossibility
  claim. A database superuser remains outside the stated application threat
  model.
- **Accidental live trading:** multiple configuration/runtime/key gates are
  intended to prevent accidental broadcasts. Their existence does not prove
  every call path or malformed configuration safe.
- **Infrastructure gaps:** provider availability and deployment operations are
  operator responsibilities. Error handling, fail-closed behavior and recovery
  remain software responsibilities and may contain defects requiring fixes.
