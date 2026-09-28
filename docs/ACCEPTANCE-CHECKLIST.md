# Handover acceptance checklist

The buyer signs off the handover by working through this list. Each item is
marked with its status at delivery:

- **[VERIFIED]** — evidence exists in the repository and was executed in the
  final freeze pass (or is a delivered fact); the buyer can re-verify with the
  stated command/procedure.
- **[PREVIOUSLY VERIFIED]** — executed on identical source in an earlier
  session; not re-executed in the final freeze sandbox.
- **[BUYER ACTION]** — must be completed by the buyer (or jointly with the
  seller) after transfer. Not a software defect.
- **[EXTERNAL]** — requires external infrastructure/services.

Statuses mirror `release-manifest.json` and `docs/HANDOVER.md` §3/§5.

## Source & identity

- [ ] **[VERIFIED]** Source received — 146 tracked files, 2.80 MB / 77,980
      lines at freeze; layout matches README §"Project layout"
      (re-check: `git ls-files | wc -l`, size table in
      `docs/BUYER-DUE-DILIGENCE.md` §A).
- [ ] **[BUYER ACTION]** Commit verified on receipt — `git log` shows freeze
      commit `0e139c3` on release commit `9c677cd`, `git status --short`
      empty (buyer-side step 2 of `docs/BUYER-DEPLOYMENT.md`).
- [ ] **[BUYER ACTION]** License assigned — insert the legal copyright holder
      into `LICENSE` (currently the deliberate placeholder "sniper-suite
      authors"; `docs/HANDOVER.md` §5.1).
- [ ] **[BUYER ACTION]** Repository URL assigned — set `repository` in the
      workspace `Cargo.toml` when publishing (intentionally absent; no fake
      URL ships).
- [ ] **[BUYER ACTION]** Security contact assigned — publish a real address
      in root `SECURITY.md` (currently points at "the repository owner's
      contact").
- [ ] **[BUYER ACTION]** Staking program ID finalized — deploy
      `programs/staking-suite` under the declared placeholder id (or change
      id + keypair and rebuild) and set `[contract] program_id`
      (`docs/STAKING.md`).

## Secrets & configuration

- [ ] **[VERIFIED]** No secrets in the delivered tree — release-check
      secret-scan gate passed; `.gitignore`/`.dockerignore` enforce; secrets
      are env-var indirections only (`docs/SECURITY.md`).
- [ ] **[BUYER ACTION]** Secrets configured outside the repo — keypair(s),
      Polymarket key, Telegram token, `API_KEY`, DB password
      (`docs/BUYER-DEPLOYMENT.md` §7).

## Infrastructure

- [ ] **[EXTERNAL]** PostgreSQL ≥ 16 configured (verified against 16.4;
      compose template delivered).
- [ ] **[EXTERNAL]** Redis 7 configured (verified against 7.2.10; compose
      template delivered).
- [ ] **[EXTERNAL]** RPC provider configured (`RPC_URL`/`WS_URL`; retry,
      failover and fan-out implemented and mock-verified).
- [ ] **[EXTERNAL]** WS/Geyser provider configured (optional
      `GEYSER_WS_URL`; poll fallback exists; real-provider e2e not executed
      in delivery).

## Functional acceptance (re-run by buyer per docs/BUYER-DEPLOYMENT.md)

- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Full local gate passed on the
      frozen tree: `./scripts/release-check.sh` → 20 PASS / 0 FAIL / 0 SKIP
      (521/521 workspace at freeze — 537/537 on the current audit-pass
      tree, db 23/23, redis 10/10, distributed 4/4,
      two-replica 1/1, staking 48/48 host at freeze — 71/71 current,
      fmt/clippy/audit/deny clean).
- [ ] **[BUYER ACTION]** Paper test completed on buyer infrastructure
      (modules enabled, simulated fills observed, dashboard live).
- [ ] **[BUYER ACTION]** Simulate test completed (`EXECUTION_MODE=simulate`:
      real transactions built + RPC-simulated, nothing broadcast).
- [ ] **[BUYER ACTION]** Health verified (`GET /health` → 200 liveness).
- [ ] **[BUYER ACTION]** Readiness verified (`GET /ready` → 200, and 503 +
      component report when a dependency is down).
- [ ] **[BUYER ACTION]** Metrics verified (`GET /metrics` → `bot_*` series
      matching README §Observability / `docs/OPERATIONS.md`).
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Backup verified — pg_dump
      procedure documented (`docs/BACKUP-RESTORE.md`); dump → restore → full
      db_integration suite green on the restored database was executed in the
      final freeze pass.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Restore verified — same
      round-trip as above; buyer should repeat once on their own PG instance.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Audit verification verified —
      `GET /api/audit/verify` recomputes the hash chain; tamper detection
      (modification/reorder/missing/duplicate) + 8-concurrent-appender
      linearity tested against real PG.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Recovery verified — restart
      replay + reconciliation + dedup tested (`db_integration`,
      `storage_lifecycle`); `recon_crash_e2e` against a local validator is
      PREVIOUSLY VERIFIED; buyer drill: `docs/BUYER-DEPLOYMENT.md` §14.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** RBAC verified — API-key
      gating, non-loopback-bind refusal without auth, Telegram
      owner/operator/readonly roles, live-mode owner-only, readonly-cannot-
      mutate regression tests (part of the 537).
- [ ] **[BUYER ACTION]** Telegram verified live — real bot token, allow-lists
      populated, refusals observed for unauthorized ids
      (`docs/BUYER-DEPLOYMENT.md` §13).

## On-chain program

- [ ] **[PREVIOUSLY VERIFIED]** Staking program compiles to BPF
      (`cargo build-sbf`, 5,440-byte `.so`, agave 2.1.21) and passed 2/2
      validator e2e (stake lifecycle, timelock, two-step admin transfer,
      genesis-mint latch) on identical source in an earlier session; CI
      `program` job re-runs both on every push once CI is active.
- [ ] **[BUYER ACTION]** External audit completed — **no external security
      audit exists**; mandatory before any mainnet deployment
      (README, `docs/SECURITY.md`, `docs/STAKING.md`).
- [ ] **[BUYER ACTION]** Genesis/admin policy set — multisig admin (Squads/
      Realms PDA recommended), timelock ≥ 24h for production, one-shot
      `GenesisMint` recipient decided (`docs/STAKING.md`).

## Operations & sign-off

- [ ] **[EXTERNAL]** Docker build + smoke executed on a real daemon (NOT
      EXECUTED in delivery sandbox; CI `docker` job covers it; compose file
      passed the `docker compose config` syntax gate statically).
- [ ] **[EXTERNAL]** CI executed on buyer's GitHub — `.github/workflows/ci.yml`
      delivered (4 jobs); no run exists from the delivery environment.
- [ ] **[EXTERNAL]** Monitoring/alerting wired — Prometheus scrape +
      `docs/OPERATIONS.md` runbook adopted.
- [ ] **[BUYER ACTION]** Production sign-off — funded live-trading validation
      under operator supervision, gradual size ramp, kill-switch drill;
      live mode requires both gates (`mode = "live"` +
      `allow_live_trading = true`) and owner-only runtime switching.

## Joint transfer actions

- [ ] **[BUYER ACTION]** Ownership transfer executed — repository, domain(s),
      provider accounts (RPC/Geyser/PumpPortal/Polymarket/Telegram bot) moved
      or re-contracted in the buyer's name (`docs/SUPPORT-HANDOVER.md`).
- [ ] **[BUYER ACTION]** Credential rotation completed — every credential
      that ever existed on either side (keys, tokens, DB passwords, API keys)
      rotated at transfer (`docs/SUPPORT-HANDOVER.md` §"Credential rotation").
