# FINAL DEPLOYMENT GUIDE — reproducible install of sniper-suite 0.1.0

Everything marked **EXECUTED** in this document was run in the delivery environment on 2026-10-31 and
its output is preserved under `/home/user/evidence/`. Everything marked **DOCUMENTED** is a procedure
for the buyer's own hosts and was *not* run here — the reason is stated each time.

Companion documents, kept because they are more detailed in their own areas: `DEPLOYMENT.md`
(development/evaluation path), `IMAGE-DIGEST-DEPLOY.md` (digest-pinned production rollouts),
`OPERATIONS-RUNBOOK.md`, `ROLLBACK-RUNBOOK.md`, `TLS-REVERSE-PROXY.md`, `BUYER-QUICKSTART.md`.

---

## 1. Prerequisites

| Component | Version used here (EXECUTED) | Notes |
|---|---|---|
| Rust | `1.98.1` (`rustup` toolchain `1.98.1-x86_64-unknown-linux-gnu`) | pinned by `rust-toolchain.toml` in the repo |
| PostgreSQL | `16.15` (cluster `16/test`) | RLS is mandatory in multi-tenant mode |
| Redis | optional | required only for the Redis-backed dedup/rate-limit path |
| Node.js | 22.x | only needed for `apps/control-plane` (Next.js) |
| Helm | `v3.16.3+gcfd0749` | chart: `deploy/helm/sniper-suite` |
| Docker | **not available in this environment** | see §8 |

Disk note from this environment: the whole workspace test matrix does not fit in 25 GB when every
test binary is linked in one process (`ld … signal 7 [Bus error]`). Build with
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2` and, if your disk is small, test crate by crate.

## 2. Environment variables that the binary actually reads

| Variable | Purpose | Used in the executed boot |
|---|---|---|
| `DATABASE_ENABLED=true` | turns the database path on | yes |
| `DATABASE_RLS_ENABLED=true` | enforces tenant RLS on every pooled query | yes |
| `POSTGRES_URL` | application pool (role `sniper_app`) | yes |
| `POSTGRES_SYSTEM_URL` | system pool (role `sniper_system`, used inside `with_system_scope`) | yes |
| `POSTGRES_DDL_URL` | migrations only (owner role `sniper`) | yes |
| `WEBHOOK_SECRET_ENCRYPTION_KEY` | base64 32-byte key for webhook-secret encryption | yes |
| `API_KEY` | bearer key for the control API; **required** when `api.bind_host` is not loopback | yes |
| `GEYSER_ENDPOINT` | enables Yellowstone gRPC ingest | yes |
| `GEYSER_X_TOKEN` | `x-token` metadata for hosted Geyser providers | documented |
| `GEYSER_PROGRAMS` | comma-separated program ids to stream | yes |
| `GEYSER_COMMITMENT` | `processed` / `confirmed` / `finalized` | yes |
| `RUST_LOG` | tracing filter | yes |
| `REDIS_URL` | Redis pool | documented |

A boot without `API_KEY` while `config.toml` has `[api] bind_host = "0.0.0.0"` is **refused on
purpose**: `refusing to bind the control API to non-loopback host '0.0.0.0' without an API key`.

## 3. Database bootstrap (EXECUTED)

```bash
# cluster (Debian layout; on a buyer host use your normal PG packaging)
pg_ctlcluster 16 test start

# roles: owner (DDL), application pool, system pool for audited background scopes
psql -p 54330 -d sniper_tcp -c "CREATE ROLE sniper LOGIN PASSWORD '…';"
psql -p 54330 -d sniper_tcp -c "CREATE ROLE sniper_app LOGIN PASSWORD '…';"
psql -p 54330 -d sniper_tcp -c "CREATE ROLE sniper_system LOGIN PASSWORD '…';"

# migrations only, no server
./target/debug/sniper-suite --migrate-only      # → 68 migrations, 119 tables
```

Verify:

```sql
SELECT count(*) FROM information_schema.tables WHERE table_schema='public';   -- 119
```

RLS facts you must not skip (`check-rls-coverage.sh` enforces them in CI):

* tenant tables are `FORCE ROW LEVEL SECURITY` with a `{public}` policy keyed on
  `app.current_org_id()`;
* `sniper_system`/`bot_system` are **not** `BYPASSRLS`, so platform queries must run inside
  `with_system_scope(...)` — merely choosing the system pool is not enough. This is what failed
  first on `tenant_runtimes` during this pass, and the fix is in
  `crates/server/src/runtime_registry/store.rs`.

## 4. Build and test (EXECUTED)

```bash
cargo fmt --all --check                              # clean (exit 0)
cargo check --workspace --all-targets                # clean
cargo clippy -p solana-kit --all-targets -- -D warnings   # clean
cargo clippy -p sniper-suite --bins -- -D warnings        # clean
cargo deny check advisories                          # 3 remaining (zstd via solana-account-decoder 2.3.13)
```

Test matrix actually executed in this pass: solana-kit 478 lib + 12 integration binaries, server 960
lib (3 ignored), bot-core 629, module-sniper 208, module-telegram 128, module-copy 65,
module-polymarket 168 lib + 76 integration (4 live tests `#[ignore]`d behind `LIVE_POLYMARKET=1`).
Full command list: `docs/FINAL-TEST-RESULTS.md`.

## 5. First boot (EXECUTED)

```bash
env DATABASE_ENABLED=true DATABASE_RLS_ENABLED=true \
    POSTGRES_URL="postgres://sniper_app:…@127.0.0.1:54330/sniper_tcp" \
    POSTGRES_SYSTEM_URL="postgres://sniper_system:…@127.0.0.1:54330/sniper_tcp" \
    POSTGRES_DDL_URL="postgres://sniper:…@127.0.0.1:54330/sniper_tcp" \
    WEBHOOK_SECRET_ENCRYPTION_KEY="$(cat key.b64)" \
    API_KEY="…" \
    GEYSER_ENDPOINT="http://127.0.0.1:50077" GEYSER_PROGRAMS="6EF8rr…F6P" \
    RUST_LOG=info ./target/debug/sniper-suite
```

Expected boot lines:

```
INFO tenant runtime registry: deployment runtime is live generation=3 action="rotated"
INFO tenant runtime registry: read-back confirmed runtime=…
INFO geyser ingest enabled (Yellowstone gRPC transaction stream) endpoint=… programs=[…]
INFO geyser ingest: subscribed endpoint=… commitment=Confirmed
```

Health check: `curl -H "x-api-key: $API_KEY" http://127.0.0.1:8080/api/health` → `{"ok":true}`.

### Split-brain protection (important on restarts)

A runtime that dies without a clean stop keeps its registry row "healthy" for `stale_after` (90 s by
default). The next boot does **not** steal the tenant: it waits for the peer's heartbeat to age out
(bounded by `stale_after + 30 s`) and then rotates, logging
`tenant runtime registry: tenant held by another live runtime — waiting for its heartbeat to go stale`.
Never delete `tenant_runtimes` rows by hand to shorten the wait.

## 6. Graceful shutdown (EXECUTED)

`SIGTERM` runs numbered phases and each logs `shutdown phase complete phase="…"`:

```
INFO sniper-suite shutting down reason=SIGTERM
INFO tenant runtime registry: deployment runtime stopped runtime=…
INFO shutdown phase complete phase="tenant-runtime-stop"
INFO geyser ingest: final counters transactions=636 reconnects=0 votes_dropped=0 last_slot=1636
INFO shutdown phase complete phase="geyser-ingest-stop"
INFO shutdown phase complete phase="db-close"
INFO sniper-suite stopped cleanly
```

After this, the runtime row is `status=stopped` with `stopped_at` set, so the next boot rotates
immediately instead of waiting for the stale window.

## 7. Kubernetes / Helm (EXECUTED — render only)

```bash
helm lint deploy/helm/sniper-suite --set image.digest="sha256:<64 hex>"
helm template sniper deploy/helm/sniper-suite --set image.digest="sha256:<64 hex>" \
     --set migrationJob.enabled=true
```

Observed here: lint `1 chart(s) linted, 0 chart(s) failed`; the render produces ServiceAccount, Secret,
ConfigMap, Service, Deployment and (with `migrationJob.enabled=true`) the migration Job, both carrying
`image: "sniper-suite@sha256:<digest>"`. With the default empty `image.digest` the render is **refused**
(`image.digest: Does not match pattern '^sha256:[0-9a-f]{64}$'`) — that is the intended fail-closed
contract, asserted by `tests/release/helm_image_contract.sh`.

The chart never accepts a mutable tag: resolve the digest of *your* built image
(`docker buildx imagetools inspect …`) and pass it. This guide does **not** ship a digest, because
inventing one would pin a pod to an image that does not exist.

## 8. Containers (EXECUTED 2026-11-01 — real image, real run)

There is no Docker daemon in the delivery environment (`/var/run/docker.sock` does not exist and
`dockerd` cannot run in this kernel-namespace-less sandbox), **but Podman 5.4.2 + crun + overlayfs
were installed and used**, so the claim "never built, never ran" no longer stands. What was executed
(`evidence/live-integration/container-build-and-run-2026-11-01.txt`):

1. `podman build` of a 9-step Containerfile whose `FROM` is **the shipped Dockerfile's own base pin**,
   `debian@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251` — the same OS
   layer a buyer's `docker build` pulls. Image: `localhost/sniper-suite:validation`, 203 MB, digest
   `c06604ae830f…`.
2. `podman run --network=host`, non-root `uid=10001(sniper)` verified with `id` inside the container,
   wired to the real PostgreSQL 16 cluster (119 tables, RLS on, separate `sniper_app` /
   `sniper_system` / DDL identities).
3. The server booted inside the container: leases acquired (`reconciliation`, `recovery`,
   `state_sync`, `accounting_maintenance`), tenant runtime registered and read back, control API
   listening, and **`GET /api/health` → 200 `{"ok":true}`** plus `GET /` → 200 dashboard.
4. `apps/control-plane/Dockerfile` — all three stages (`npm ci`, `npm run build`, standalone runtime) —
   built the same way; the container serves the dashboard as `uid=1000(node)`, `GET /` → 200.
5. **The stack itself was brought up**: official `docker compose` v2.29.7 against podman's
   Docker-compatible API, using the new `deploy/compose/docker-compose.host-network.yml` overlay (this
   kernel cannot create a bridge: `netavark: nft did not return successfully`). Postgres 16 + Redis 7
   healthy from their digest pins, **68 migrations applied**, bot `/api/health` 200, dashboard 200.
   See `evidence/live-integration/compose-stack-2026-11-01.txt`.

**What this does *not* prove, stated plainly:** the server image was built from the **debug** profile
binary (106 MB) because a release build of the 675-crate graph does not fit in the 3 GB free on this
disk, so the shipped `Dockerfile`'s release stage was not exercised end-to-end; the compose run used the
host-network overlay because this kernel cannot create a bridge, and one service (`postgres-roles`) had
to be reproduced with `podman run` because podman's Docker-compatible API mangles compose's `$$` escaping.
Nothing above asserts a production image or a production deployment. The static asset gate remains in force as well: `scripts/validate-container-assets.py` →
**83 passed, 1 warned (documented fixed password in the localhost-only test stack), 0 failed**.

On a Docker host, the documented path is unchanged:

```bash
docker compose -f docker-compose.yml build
docker compose -f docker-compose.yml up -d
docker compose -f docker-compose.test.yml up -d --wait    # integration stack
```

## 9. Backups, restore, rollback

* `scripts/backup-postgres.sh` / `scripts/restore-pitr.sh` + `deploy/compose/docker-compose.pitr.yml`
  for WAL archiving and point-in-time recovery (RPO minutes instead of a backup interval).
* `scripts/prune-wal-archive.sh` prunes segments the oldest retained base backup no longer needs.
* Rollback: `ROLLBACK-RUNBOOK.md` + `scripts/deploy-release.sh`.

## 10. What this guide does not cover

Stripe, Vault/KMS/HSM, Telegram, a funded Solana/Polygon wallet and mainnet RPC have **no
credentials here**; those integrations are documented in `BUYER-HANDOVER.md` and remain
external-credential rows. No live order was ever placed from this environment.
