# Final known-limitations register (buyer handover)

Every limitation of the 0.1.0 delivery, in exactly one of six categories.
Categories are never mixed: a security blocker is never filed under "future
enhancement", and nothing blocked by the vendor's environment is claimed as
verified. Cross-references: `docs/EVIDENCE-INDEX.md` (proof per claim),
`docs/HANDOVER.md` §3 (verification taxonomy), acceptance test (buyer
re-verification fields).

## 1. VERIFIED (executed with artifacts — see evidence index)

* Workspace tests 537/537 (and `--all-features` 537/537) against real
  PostgreSQL 17.11 + Redis 8.0.2; gated db/redis/distributed/two-replica
  suites executed, not skipped.
* Staking host tests 71/71 on the final source; clippy `-D warnings` clean
  (app + program).
* `cargo build-sbf` artifact 187,504 B, SHA-256 `57a890fa…`; byte-identical
  on incremental AND full cold rebuild (fresh toolchain + platform-tools).
* Validator e2e 3/3 on a real Agave 2.1.21 BPF VM, including funded
  stake→reward→claim→unstake, exact/over-cap supply boundaries, metadata vs
  the REAL mainnet-cloned mpl-token-metadata, replay rejection, authority.
* DB backup→restore round-trip: dump `5989ecf1…`, tables/migrations/
  rowcounts identical ×3, db_integration 23/23 on the restored DB, app
  started/served/shut-down-cleanly against it.
* App smoke: /health, /ready (4 components), /api/status (paper,
  live_allowed=false, kill_switch=false), /metrics, clean SIGTERM drain.
* Final release gate: release-check 20 PASS / 0 FAIL / 0 SKIP;
  verify-delivery 7 PASS / 0 FAIL; cargo audit ×2 (0 errors, 9 allow-listed
  warnings); cargo deny (advisories/bans/licenses/sources ok).
* latency_bench read-only + simulate legs executed vs public devnet
  (machine-readable record with environment caveats).
* Static security scans: 0 unsafe, forbid(unsafe_code) ×4, 0 markers,
  0 secret-pattern hits, 0 keypairs in tree.
* Source completeness: FINAL 183-file tree four-way byte-verified (repo /
  package mirror / tarball extraction / provenance ledger — 0 missing,
  0 extra, 0 mismatched) + per-file provenance hashes; independently
  re-verified by the 2026-09-19 adversarial audit (fresh extraction +
  per-file re-hash). The earlier 175-file three-way check remains valid for
  the hardening-pass baseline (labeled historical).

## 2. NOT EXECUTED (never run anywhere — honestly labeled, wired for buyer)

* Funded live trading on any venue (Polymarket orders with real collateral;
  Solana broadcasts with real funds). Simulation ≠ funded; paper ≠ live.
* Landing-rate benchmark leg (requires funded keys + explicit approval).
* Real Geyser provider end-to-end (mock-tested; provider is buyer-supplied).
* Real Telegram bot session (regression-tested against mocks; no vendor
  token exists).
* Multi-replica beyond 2 (two_replica_mirror executes 2 real processes;
  larger topologies untested).
* GitHub Actions run (see also category 3 — no runner existed).

## 3. BLOCKED BY ENVIRONMENT (vendor sandbox lacked the resource)

* Docker image build, `docker compose config -q`, container smoke — no
  Docker daemon. Native-equivalent binary smoke PASSED and is labeled as
  NOT a container run. Buyer: acceptance G1 / CI docker job.
* Actual CI execution — no GitHub runner. Every CI step has an executed
  local 1:1 equivalent (`docs/CI-LOCAL-EQUIVALENCE.md`); local ≠ Actions run.
  Buyer: acceptance R2.
* Mainnet deployment of anything — out of scope for the vendor by policy
  (no real program id, no funded keys, no approval).

## 4. REQUIRES BUYER (operator decisions/assets/actions)

* **Final staking deployment identity:** generate the program keypair,
  `staking-identity.sh set-id`, rebuild, deploy (script refuses
  keypair≠declare_id and placeholder-on-public-cluster). The declared id
  `3vEEMM…9mfy` is a PRE-DEPLOYMENT PLACEHOLDER; the program is NOT
  deployed anywhere; the vendor holds no keypair for it.
* Upgrade authority, payer/fee authority, and cluster choice for the
  program deployment.
* All credentials: Solana wallet keypair(s), Polygon funder key, Polymarket
  API credentials, Telegram bot token + owner chat id, API keys, PG/Redis
  connection strings.
* Live-mode activation decision (`allow_live_trading` + owner key) and the
  staged funded validation of `docs/LIVE-VALIDATION.md` (§O acceptance).
* Legal/identity fill-ins: LICENSE copyright holder, repository URL,
  security contact (`docs/HANDOVER.md` §5).
* TLS termination, backup schedule, monitoring/alerting wiring in the
  buyer's infrastructure.

## 5. REQUIRES EXTERNAL PROFESSIONAL

* **External security audit — none exists; none is claimed.** Strongly
  recommended before mainnet deployment of the staking program (immutable
  on-chain surface) and before funded live operation.
* Legal review: license/IP transfer paperwork, MIT copyright-holder
  substitution, regulatory review of trading operations in the buyer's
  jurisdiction(s).
* Optional: penetration test / fuzzing campaign (not performed).

## 6. FUTURE ENHANCEMENT (not defects; scoped out by design)

* Signer backends vault/kms/hsm: currently FAIL STARTUP by design
  (local-keypair-only signer registry). Integrating a real HSM/KMS is a
  scoped future project, not a configuration switch.
* Six tracked dependency-deprecation allows (documented in
  `docs/HANDOVER.md`; harmless today, must be resolved at the next
  dependency bump — policy: any dependency change requires re-running the
  full gate).
* Redis 8 / PostgreSQL 17 were the evidence-run service versions; compose
  pins postgres:16-alpine + redis:7-alpine — newer-major validation is a
  future matrix item (current runs were on NEWER versions than pinned, so
  the pinned floor is expected-safe, not proven-exact).
* Dashboard is a single-file HTML (no build pipeline); richer UI is future
  scope.
* Protocol-drift adaptation tooling (e.g., automated layout re-learning
  beyond `LayoutStore`) — future scope; drift risk is registered in
  `docs/BUYER-RISK-REGISTER.md`.

## Explicit non-claims (policy)

No production deployments, customers, revenue, ROI, transaction volume,
valuation, security certification, latency guarantee, profit guarantee,
execution guarantee, or mainnet-proven status is claimed anywhere in this
delivery. Bench figures are measurements of the documented sandbox
environment, not product guarantees. Historical (freeze-era) evidence stays
labeled historical and is never re-presented as current.
