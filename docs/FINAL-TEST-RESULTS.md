# FINAL TEST RESULTS — execution record

Every row below was produced by running the command in this environment on 2026-10-31.
Nothing here is projected, estimated or copied from an earlier report. Where a command could
not run, the row says `BLOCKED` with the reason.

## Fifth pass — re-execution log (2026-10-31, later the same day)

The dependency graph changed after the earlier rows were recorded: `k256` 0.13.4 → 0.14.0 (removes
RUSTSEC-2026-0387) and the legacy `solana-client` crate was replaced by `solana-rpc-client` 2.3.13
(removes RUSTSEC-2026-0346/0349/0381/0382 — rustls 0.21.12 → 0.23.46, tungstenite 0.20.1 → 0.24.0).
Everything that depends on those crates was therefore re-tested:

| Command | Result |
|---|---|
| `cargo test -p solana-kit --lib --tests` | 478 lib passed + 12 integration binaries passed, 0 failed |
| `cargo test -p sniper-suite --lib` | 960 passed, 0 failed, 3 ignored |
| `cargo test -p bot-core --lib` | 629 passed, 0 failed |
| `cargo test -p module-sniper --lib` | 208 passed, 0 failed |
| `cargo test -p module-telegram --lib` | 128 passed, 0 failed |
| `cargo test -p module-copy --lib` | 65 passed, 0 failed |
| `cargo test -p module-polymarket` | 168 lib + 76 integration passed, 4 live ignored |
| `cargo clippy -p solana-kit --all-targets -- -D warnings` | clean |
| `cargo clippy -p sniper-suite --bins -- -D warnings` | clean |
| `cargo deny check advisories` | 8 → 3 remaining at the time (all `zstd`); **now 0** — see *Seventh pass* below |
| `cargo test --workspace --all-targets` | **could not complete here**: linking every test binary in one process filled the 25 GB disk and the linker died with `signal 7 [Bus error]`. The suite above was therefore run crate by crate with `cargo clean -p` between runs. |

New coverage added in the same pass: `crates/solana-kit/src/geyser.rs` +
`tests/geyser_ingest.rs` (Yellowstone gRPC ingest, 4 + 3 tests) and
`crates/server/src/solana/geyser_ingest.rs` (boot wiring, proven against
`examples/geyser_mock_server.rs`: 636 transactions, clean SIGTERM).

### Seventh pass — the last three advisories closed (2026-10-31)

`RUSTSEC-2026-0391 / 0398 / 0403` were all `zstd 0.13.3` / `zstd-safe 7.3.0`, reachable only through
`solana-account-decoder{,-client-types}` 2.3.13. Both crates are now **vendored** under `vendor/` with
their single `zstd` requirement bumped to `0.14.3`; no Rust source in them changed and the API the
workspace compiles against is identical. `cargo deny check` → **`advisories ok, bans ok, licenses ok,
sources ok`** (0 unignored advisories; 3 advisory IDs remain ignored with written rationale in `deny.toml` — the dalek chain via solana-keypair, and bincode 1.x).

Re-run after the patch: solana-kit 478 lib + 20 integration binaries, module-copy 65 lib + integration,
module-sniper 208 lib + integration, module-telegram 128, sniper-suite 960 (3 ignored) — all passed,
`cargo fmt --all --check` clean, clippy clean.

## Eighth pass — LIVE integrations executed against real services (2026-11-01)

Every row below was produced in this environment on 2026-11-01 by running the named
command against a **real** external service. Nothing here was simulated by a stub
written for this repository: Vault is HashiCorp's own server, Stripe is Stripe's own
`stripe-mock`, Solana is the real public cluster, Telegram is the real Bot API.

| # | What ran | Command | Result | Evidence |
|---|---|---|---|---|
| 1 | **Vault transit signing — real server** (`vault server -dev`, transit engine, ed25519 key) | `VAULT_ADDR=… VAULT_TOKEN=… VAULT_TRANSIT_KEY=<key name> cargo test -p sniper-suite --test vault_live_transit -- --ignored` | **4/4 pass** — `sys/health`, token permission, key metadata, resolve → sign → verify (signature verifies against Vault's own pubkey, fails on a different message, is deterministic), and fail-closed on pinned-pubkey mismatch, missing key, rejected token | `evidence/live-integration/vault-live-transit-2026-11-01.txt` |
| 2 | **Stripe Checkout Sessions over real HTTP** (Stripe's own `stripe-mock` 0.206.0) | `STRIPE_MOCK_URL=http://127.0.0.1:12111 cargo test -p sniper-suite --test live_stripe_mock -- --ignored` | **4/4 pass** — `POST /v1/checkout/sessions` decoded into a session id + hosted URL, `Idempotency-Key` replay accepted, `LIVE_BILLING` gate refuses without opt-in, webhook HMAC verifies and rejects tampered / wrong-secret / unsigned / stale-replay | `evidence/live-integration/live-stripe-mock-2026-11-01.txt` |
| 3 | **Solana mainnet-beta read-only** (real public cluster) | `SOLANA_LIVE_RPC=https://api.mainnet-beta.solana.com cargo test -p solana-kit --test live_mainnet_readonly -- --ignored` | **4/4 pass** — live `getVersion` (4.3.2) / `getHealth` / slot 463,936,752 / block height / fresh blockhash reported valid / USDC mint decodes to **6 decimals**, owner `Tokenkeg…`, 82-byte data / lamport balance 558,550,303,855 / absent account returns `None` | `evidence/live-integration/live-mainnet-readonly-2026-11-01.txt` |
| 4 | **Telegram Bot API** (real `api.telegram.org`) | `cargo test -p module-telegram --test live_telegram_api -- --ignored` | **3/3 pass** (1 authenticated case explicitly SKIPPED) — the real `401 Unauthorized` is surfaced as a typed error and the bot token never appears in the error text | `evidence/live-integration/live-telegram-api-2026-11-01.txt` |
| 5 | **Devnet money path** | `SOLANA_LIVE_DEVNET_RPC=https://api.devnet.solana.com cargo test -p solana-kit --test live_devnet_transaction -- --ignored` | **2/2 pass, faucet-blocked** — the public faucet refused all 4 airdrop attempts (rate limit) so **no funds were fabricated and nothing was sent**; the negative control ran live: a real signed transfer from an unfunded account **never confirmed** | run log in the same session; test file `crates/solana-kit/tests/live_devnet_transaction.rs` |
| 6 | **Container assets against third-party tools + the live registry** | `bash tests/release/container_assets_live.sh` | **PASS** — hadolint 2.12.0: both Dockerfiles clean; real `docker compose config -q`: base file, test file, demo stack, base+backup, base+backup+PITR, base+TLS all valid, and the base file **refuses** to render without its required env (fail-closed); all **7 digest-pinned** base images resolve on `registry-1.docker.io` with a `linux/amd64` manifest (2 tag-only test-compose refs also resolve) | `evidence/live-integration/container-assets-live-2026-11-01.txt` |

### What this pass still does not claim

* ~~No container image was built and no container was run~~ — **overtaken by the ninth
  pass below**: an image was built with Podman and the server ran in it, serving HTTP.
  The one part still unproven is the shipped Dockerfile's *release* stage.
* **No live Stripe account, no real money, no delivered webhook.** `stripe-mock`
  is Stripe's own mock, not Stripe.
* ~~No funded wallet~~ — **overtaken by the ninth pass below**: a funded wallet was
  created on a local Agave validator and a real transfer landed. Mainnet fills still
  require capital and are not claimed.
* **No authenticated Telegram bot.** A real bot token would enable the skipped case.
* **No independent security audit.** Unchanged, and not claimable.
* **AWS/GCP KMS and HSM signing is not implemented in this tree** — those providers
  are validated references that fail closed; the shipped remote-signing integration
  is Vault, and that one is now live-proven.

## Ninth pass — the three items that were still open, actually executed (2026-11-01)

This pass exists because "handled another way and skipped" is not an acceptable outcome.
All four rows below were executed on this machine; each has a transcript under
`evidence/live-integration/`.

| # | What ran | Command | Result | Evidence |
|---|---|---|---|---|
| 7 | **Funded wallet + real transfer on a real Solana validator** (Agave 4.3.2 `solana-test-validator`, not a mock) | `SOLANA_LIVE_DEVNET_RPC=http://127.0.0.1:8899 cargo test -p solana-kit --test live_devnet_transaction -- --ignored` | **2/2 pass** — the wallet was funded by a real airdrop of 1 000 000 000 lamports on the validator's own faucet, the transfer **landed in slot 1289** (`fee=5000`, confirmed by the cluster, not by the test), and the unfunded negative control was refused. First run failed honestly: `SEND_LAMPORTS = 25_000` → `insufficient funds for rent`; the constant is now `2_000_000` (rent-exempt) with the reason in a comment | `evidence/live-integration/local-validator-transfer-2026-11-01.txt` |
| 8 | **Real container builds + real container runs** (Podman 5.4.2 + crun) | `podman build` → `podman run --network=host` | **PASS** — server image: 9-step Containerfile `FROM` the shipped Dockerfile's own pin `debian@sha256:3783cc…`, 203 MB, non-root `uid=10001`, wired to real PostgreSQL 16, **`GET /api/health` → 200 `{"ok":true}`**; control-plane image: the shipped 3-stage `apps/control-plane/Dockerfile` (`node@sha256:43ac6c60…`, `npm ci`, `npm run build`, `.next/standalone`), non-root `uid=1000(node)`, `GET /` → 200. Caveat: the server binary is the **debug** profile (a release build does not fit in the free disk) | `evidence/live-integration/container-build-and-run-2026-11-01.txt` |
| 8b | **The stack, up** — official `docker compose` v2.29.7 driving podman's Docker-compatible API | `docker compose -f docker-compose.yml -f deploy/compose/docker-compose.host-network.yml up -d` | **PASS with one exact exception** — Postgres 16 + Redis 7 brought up from their digest pins and healthy; **68 migrations applied** to the fresh volume (`FORCE RLS enabled on 86 tenant table(s)`); bot `/api/health` → 200 with the keypair mounted from the host; Next.js `GET /` → 200. `postgres-roles` exited 2 because compose's `$$` escaping is passed through unchanged by podman's compat API (the identical command via `podman run` succeeds — proven in the log); the roles were created with those same statements. Two defects found and fixed: `docker-compose.yml` never forwarded `API_KEY` (bot refused to boot on a fresh `.env`), and `REDIS_URL` is inert under the shipped `[redis] enabled = false` — now documented in place | `evidence/live-integration/compose-stack-2026-11-01.txt` |
| 9 | **Third-party secret scan** — gitleaks 8.28.0 | `bash tests/release/secrets_third_party.sh` (new gate) | **PASS** — baseline 37 findings on the delivered tree, every one triaged by hand (test placeholders, public on-chain identifiers, the custody detector's own PEM markers) and encoded as a *reasoned* allowlist in `.gitleaks.toml`; scan of the delivered tree now reports **no leaks found**. A new secret still fails: the allowlist names individual strings and test paths, never production source wholesale | `evidence/live-integration/gitleaks-2026-11-01.txt` |
| 10 | **Third-party dependency scan** — osv-scanner 1.9.1 | `osv scan --lockfile Cargo.lock --lockfile apps/control-plane/package-lock.json` | **RUN, and it caught a real error in our own reporting.** 675 Rust / 540 npm packages → 12 Rust + 7 npm advisories. The 12 Rust entries split into: 3 inside `deny.toml`'s written ignore rationale (ed25519-dalek, curve25519-dalek, bincode), 1 inside `.cargo/audit.toml`'s (rsa, the never-compiled `sqlx-mysql` optional driver), and 8 unmaintained-transitive advisories that the `unmaintained = "workspace"` policy accepts by design. The 7 npm entries are all **dev-only** test tooling (`vite`, `vitest`, `tinypool`, `esbuild`, `zod`, `minimatch`, `@vitest/mocker`) — none is in the production image. **What it exposed:** this document and two others claimed the `deny.toml` ignore list was *empty*; it is not — it holds 3 reasoned entries. That claim is corrected here and in `FINAL-DELIVERY-REPORT.md` / `FINAL-CODE-COMPLETENESS-AUDIT.md` | `evidence/live-integration/osv-scanner-2026-11-01.txt` |

### The single command this project had never completed — now it runs, and it found five real defects

`cargo test --workspace --all-targets` is the one command earlier passes could not finish: the
7 GB `target/debug/incremental` directory filled the 25 GB disk and the linker died with
`signal 7 [Bus error]`. It runs now — `CARGO_INCREMENTAL=0` plus `CARGO_PROFILE_TEST_DEBUG=0`
(no debuginfo in ~180 test binaries) is all it needed — every target compiles and links in one
`cargo` invocation, and running it surfaced **five pre-existing defects that crate-by-crate runs
could not see**:

| Found by the full run | Fix |
|---|---|
| `crates/core/tests/db_integration.rs` — the three audit-chain tests assert a *global* chain property while each appends and then deletes its own rows; in parallel, one test's cleanup reads as a chain break in another (`Some(19)` vs `None`) | a process-wide mutex serialises the three (`AUDIT_CHAIN_TEST_LOCK`), no new dependency |
| `crates/module-sniper/tests/replay.rs` — `regenerate_replay_fixtures` truncated the fixture files with `fs::write` while two other tests read the same directory → `EOF while parsing a value at line 1 column 0` | write-then-rename, so a reader sees old or new bytes, never an empty file |
| `crates/server/tests/batch6_transaction_readiness.rs` — pinned `docs/*.md == 160`; the tree has **167** (the pin was never bumped when the delivery documents landed) | pin updated to 167 with the date, the reason, and the note that it must match `docs_files` in the release manifest |
| `crates/core/src/session/token.rs` — `!plaintext.starts_with('_')` was **flaky (~1 run in 64)**: the base64url body can itself start with `_` | assertion replaced by a length/containment assertion |
| …and behind that flaky assertion, a **real defect**: `public_prefix()` splits the namespace at the *first* `_`, so a body containing `_` produced a prefix that spanned a namespace that does not exist | `generate_token` maps `_`→`-` in the body, so the only `_` in a token is the namespace separator |

**One item is still open and is reported, not papered over:** in the full run,
`module-copy/tests/two_replica_mirror.rs::two_replicas_one_whale_trade_one_execution` asserted
`exactly one replica may pass the claim gate` and both replicas got past it (both then died at the
RPC). The same test, same database, run alone: **6 runs, 6 passes**. It looks load/ordering
sensitive. A copy-trading claim gate that can admit two replicas is money-relevant, so it stays on
the open list (see `KNOWN-LIMITATIONS.md`) rather than being explained away. Transcript of all five
runs: `evidence/live-integration/full-workspace-tests-2026-11-01.txt`.

**What the ninth pass still does not claim.** No live Stripe account and no real money; no
authenticated Telegram bot; no AWS/GCP KMS or HSM signer (the code validates the provider
references and fails closed — implementing a KMS client is a feature, not a verification step);
no production deploy; no human audit. And one observed warning is reported rather than hidden:
during the container boot the startup audit record logged
`AUDIT PERSISTENCE FAILED … new row violates row-level security policy for table "audit_events"`
and fell back to the ring buffer + journal. The server is designed to continue in that case, but the
RLS policy for the *startup* audit actor is worth a look on a buyer's cluster.

## Environment

| Item | Value |
|---|---|
| OS | Debian GNU/Linux 13 (trixie), x86_64 |
| Memory / disk | 1 982 MB RAM, 25 GB disk (8 GB swapfile added; `-j2` was OOM-killed, so builds used `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`) |
| Rust | `rustc 1.98.1` / `cargo 1.98.1` — exactly what `rust-toolchain.toml` pins; `rustfmt 1.9.0`, `clippy 0.1.98` |
| PostgreSQL | 17.11 local on `127.0.0.1:54329` (project targets `postgres:16-alpine`; no Docker available) |
| Redis | 8.0.2 local on `127.0.0.1:63799` |
| Node | `npm ci` from the committed lockfile; Chromium headless shell 1243 |
| Docker | **not available** — the compose path could not be exercised |

## Rust

| Command | Result | Evidence |
|---|---|---|
| `cargo fmt --all -- --check` | **PASS** (exit 0) | clean |
| `cargo check --workspace --all-targets --locked` | **PASS** (3 m 44 s) | `Finished dev profile` |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (4 m 35 s) | zero diagnostics |
| `cargo test --workspace --lib -- --test-threads=1` (before the Batch 02 code changes) | **PASS** — 2 651 passed / 0 failed / 3 ignored | 8 lib binaries |
| `cargo test --workspace --lib -- --test-threads=1` (after the Batch 02 code changes) | **PASS** — 629 + 65 + 159 + 208 + 128 + 34 + 954 + 474 = **2 651 passed**, 0 failed, 3 ignored | re-run to prove no regression |
| `cargo build -p sniper-suite --bins` | **PASS** — binary 107 994 176 bytes | used for the live boot checks |

## PostgreSQL + Redis integration (real services, `REQUIRE_SERVICES=1`)

`bash scripts/test-with-services.sh --existing` — run twice (before and after the code
changes); **exit 0 both times**.

| Suite | Passed | Failed | Ignored |
|---|---:|---:|---:|
| `rls_coverage` (applies all 68 migrations, audits the RLS catalogue) | 1 | 0 | 0 |
| `trusted_proxy_client_ip` | 4 | 0 | 1 (by design) |
| `trusted_proxy_client_ip --ignored` | 1 | 0 | 0 |
| `rls_isolation` | 1 | 0 | 0 |
| `rls_pool_routing` | 1 | 0 | 0 |
| `redis_saas_integration` | 5 | 0 | 0 |
| `redis_integration` (bot-core) | 10 | 0 | 0 |
| `multi_replica_durable_state` | 13 | 0 | 0 |
| `custody_db_authoritative` | 1 | 0 | 0 |
| `rls_background_paths` | 1 | 0 | 0 |
| **Total** | **38** | **0** | **1** |

Additional: `scripts/check-rls-coverage.sh` (ephemeral PostgreSQL, migrations applied twice)
→ **PASS**. `cargo test -p sniper-suite --test dashboard_csp` → **1 passed**.

## Live runtime checks (this batch's new evidence)

Booted the compiled binary against a **brand-new** database (`sniper_boot3`), with
`DATABASE_ENABLED=true DATABASE_RLS_ENABLED=true DATABASE_REQUIRED=true` and real
`sniper_app` / `sniper_system` login roles:

| Observation | Result |
|---|---|
| Startup with a database | **PASS** — previously aborted: `duplicate key value violates unique constraint "organizations_slug_key"` (reproduced on the pristine binary too) |
| `DCA schedule worker started` | **PASS** — `worker=dca-local-89027` |
| Audited system scope for tenant discovery | **PASS** — `background system scope role="bot_system" reason="discover organizations for DCA scheduling"` |
| `GET /api/health` | **200** `{"ok":true}` |
| `GET /` (dashboard) | **200**, 6 525 bytes |
| `GET /dashboard/app.js` | **200**, 6 702 bytes, `application/javascript; charset=utf-8` |
| `Content-Security-Policy` response header | present, `script-src 'self'` |
| Shutdown | clean on SIGTERM (no error lines in the log) |

## Batch 02 follow-up run (after the container reset)

The container this workspace runs in was reset between passes: `.git`, the Rust toolchain,
`target/` and both services were gone (the working tree and the packaged ZIP survived intact).
Everything below was re-established and re-run from scratch, so these are independent
reproductions rather than repeats of the earlier numbers.

| Step | Result |
|---|---|
| Container state | `.git`, `~/.cargo`, `target/`, PostgreSQL, Redis all absent; working tree complete (23 MB) |
| Repository restored | `git clone` upstream + `.git` transplanted; `HEAD 4a59411` on `main`; working tree re-staged (77 modified, 5 deleted, 4 added) |
| Toolchain | `rustup` + pinned `1.98.1` (`rustfmt`, `clippy`) reinstalled |
| Services | PostgreSQL 17.11 on `:54329` (roles `sniper`, `sniper_app`, `sniper_system`), Redis on `:63799` |
| Memory | 1 982 MB RAM; the final crate link was OOM-killed at `-j2`, so a 6 GB swapfile was added and the build ran `-j1` |
| `cargo build -p sniper-suite --bins --locked` | **PASS** — 108 082 744-byte binary |
| `cargo fmt --all -- --check` | **PASS** |
| `cargo clippy -p sniper-suite --bins --locked -- -D warnings` | **PASS** (one real lint found and fixed: `unwrap_or_else(OrganizationId::new)` → `unwrap_or_default()`, behaviour-identical because `Default for OrganizationId` is `new()`) |
| `cargo test -p sniper-suite --lib` | **PASS — 954 passed, 0 failed, 3 ignored** (same count as the pre-reset baseline) |
| `cargo test -p sniper-suite --test rls_background_paths` (live PG + Redis) | **PASS — 1 passed, 0 failed** |
| Live boot on a fresh database (`sniper_lc`) | **PASS** — `tenant lifecycle worker started`, `/api/health` 200 |
| Lifecycle worker end-to-end | **PASS** — an inserted job advanced `requested → trading_disabled → credentials_revoked → sessions_invalidated` and parked at `waiting` within ~30 s |

## Frontend — re-run in the final environment (after the container reset)

`node_modules` was destroyed by the reset, so the whole frontend chain was re-established and
re-executed rather than quoted:

| Command | Result |
|---|---|
| `npm ci --no-audit --no-fund` | **PASS** — 355 top-level packages from the committed lockfile |
| `npx tsc --noEmit` | **PASS** — 0 diagnostics |
| `npx vitest run` | **PASS — 34 files, 244 tests passed**, 0 failed (26.9 s) |
| `npx eslint .` | **PASS** — 0 errors, 0 warnings |
| `npm test` (node test runner) | **PASS — 14 passed**, 0 failed |
| `npm run build` (`next build`) | **PASS** — production bundles, 73 routes |
| `npx playwright test` | **PASS — 69 passed**, 0 failed (1.5 min), after `playwright install-deps chromium` + headless-shell 1243 |

## Third sweep — the previously "NOT RUN" rows, now executed

Everything below was installed and executed in this environment after the reset.

| Check | Result |
|---|---|
| **PostgreSQL 16 matrix** (the exact major version `docker-compose.test.yml` targets) | Installed `postgresql-16` 16.15 from PGDG; cluster on `:54330`; **68/68 migrations applied, 119 public tables**; full service suite re-run → **10 binaries, 38 passed, 0 failed, 1 ignored** |
| **Staking program — native tests** | `cargo test` in `programs/staking-suite` → **82 passed** (71 + 8 + 3), 0 failed |
| **Staking program — on-chain build** | `cargo build-sbf` → `staking_suite.so`, **164 488 bytes**, `ELF 64-bit, e_machine = Linux BPF` — a real deployable SBF artifact |
| **Staking program — formatting** | `cargo fmt --check` → clean |
| **`cargo deny check advisories`** | **18 findings → 9** after `cargo update` (166 crate bumps). Fixed: RUSTSEC-2026-0385, -0386, -0347, -0348, -0410 and three `-0381` duplicates. *Superseded: 9 → 3 in the fifth pass, 3 → 0 in the seventh pass.* |
| **`cargo update`** | `Cargo.lock` moved 166 versions; the workspace was rebuilt and the suites re-run against it (see the counts above) |
| **Limit-order worker — live** | Boot log `limit-order worker started worker=limit-order-local-67756`; a real order reached `triggered` off a **live Jupiter mark** and produced an idempotent `execution_intents` row `limit-order:lo-e2e-1` (`pending`, no fill claimed) |
| **Limit-order worker — fail-closed** | An order on a non-existent mint stayed `active`, `attempts=0`, **no intent written** — a missing mark never fires an order |
| **Historical batch gates** | Now report `SKIPPED` with a printed reason instead of a permanent false failure (5 later commits exist after their `BASE_COMMIT.txt`; `FILES-MANIFEST.txt` was a per-batch delivery artifact) |

### Still not executable here

| Check | Status | Reason |
|---|---|---|
| Docker / `docker-compose.test.yml` | **BLOCKED** | no daemon in this sandbox; the `--existing` path is the CI path and was run instead |
| Live providers (Stripe, Vault/KMS/HSM, CLOB, Telegram, mainnet RPC) | **BLOCKED** | no credentials, no funded wallet, no provider account |
| Independent security audit / penetration test | **NOT PERFORMED** | no auditor engaged; none claimed |
| Remaining 9 RustSec advisories | **CLOSED** | was open when this table was written; all 9 were closed in the fifth and seventh passes — `cargo deny check` is now fully green |

## Fourth sweep — copy-follow wiring and the full workspace re-run (2026-10-31)

Executed after `CopyFeedClient` (`copy_feed.rs`), `PolyBot::copy_poll_once`
(`copy_run.rs`) and the `Tick::Copy` arm were added, and after the seven new
`[polymarket]` copy keys were introduced into `PolymarketConfig`.

| Check | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | **PASS** (clean; it first flagged the new code and the rewrite was applied with `cargo fmt --all`) |
| Lints, every target | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** — 0 errors, 0 warnings (it caught `clippy::items_after_test_module` in `config.rs`; the helpers were moved above the test module rather than allowed) |
| Workspace unit tests | `cargo test --workspace --lib` | **PASS — 2 662 passed, 0 failed, 3 ignored** (629 core + 956 server + 168 polymarket + 208 + 474 + 128 + 65 + 34) |
| Polymarket integration matrix | `cargo test -p module-polymarket` | **PASS — 76 tests, 0 failed** across 11 binaries, including the new `tests/copy_follow.rs` (**8 passed**) |
| Copy-follow list | `cargo test -p module-polymarket --test copy_follow` | **PASS — 8 passed, 0 failed** in 0.02 s (mock venue over real HTTP; no external network) |
| Core config contract | `cargo test -p bot-core --lib` | **PASS — 629 passed** (includes `bundled_example_config_parses`, which now asserts the seven copy keys are documented in `config.toml.example`) |
| Repository gates | `make gates` | **PASS** — `verify-delivery.sh` 14 PASS / 0 FAIL, migrations, OpenAPI coverage, routes-vs-UI, secret scan 0 findings, helm image contract, marketing-claims gate (including its planted-claim rejection test) |
| Stats / manifest currency | `tests/release/stats_current.sh`, `tests/release/manifest_current.sh` | **PASS** — 93 stat markers current; manifest matches the tree |

What this does **not** prove: the copy path was never executed against
Polymarket's real data API or CLOB. The evidence is the in-process mock
(real HTTP, real client/signing/pipeline code), so the "live trading
unverified" banner stands unchanged.

## Frontend

| Command | Result |
|---|---|
| `npx vitest run` | **PASS** — 34 files, **244 passed**, 0 failed |
| `npx vitest run <the 8 batch page-test files>` | **PASS** — 8 files, **66 passed** |
| `npx tsc --noEmit` | **PASS** (0 diagnostics) |
| `npx eslint .` | **PASS** — 0 errors, 0 warnings |
| `npm test` (node test runner) | **PASS** — 14 passed, 0 failed |
| `npm run build` (`next build`) | **PASS** — 73 routes, production bundles |
| `npx playwright test` | **PASS** — **69 passed**, 0 failed (was 68/1 before the flaky-locator fix) |

## Repository gates

| Command | Before this batch | After |
|---|---|---|
| `make gates` (fake-data, migration graph, script modes, delivery, OpenAPI coverage, routes↔UI, secrets, helm contract, claims) | `verify-script-modes.sh` FAIL | **exit 0, all PASS** |
| `scripts/verify-script-modes.sh` | FAIL — 22 offenders | **PASS — 0 offenders** |
| `scripts/verify-delivery.sh` | **9 PASS / 5 FAIL** | **14 PASS / 0 FAIL** |
| `scripts/scan-secrets.sh` | — | PASS (0 findings) |
| `scripts/verify-migration-graph.sh` | — | PASS |
| `tests/release/stats_current.sh` / `manifest_current.sh` / `evidence_truth.sh` / `marketing_claims.sh` / `helm_image_contract.sh` / `forensic_sql_extractor.sh` | — | **all PASS** |
| `tests/release/buyer_source_parity.sh` | BLOCKED (no tree) | **PASS** — `identical=1591 missing=0 stale=0 differs=0` |
| `scripts/build-release-package.sh` | — | **exit 1 — BLOCKED**: `LICENSE` still contains `[SELLER LEGAL ENTITY NAME]` (guard 5, by design) |
| `scripts/verify-buyer-package.sh` | — | **exit 1** — downstream of the blocked package build |

## Commands that could not run

| Command | Status | Reason |
|---|---|---|
| `docker compose -f docker-compose.test.yml up` | **BLOCKED** | no Docker daemon; `--existing` mode used instead (the CI path) |
| PostgreSQL **16** matrix | **BLOCKED** | only 17.11 available without Docker |
| `cargo test --workspace` (full 60+ integration binaries) | **NOT RUN** | each linked binary is ~150 MB; disk could not hold the matrix. `--lib` + the 9 mandated service suites + `dashboard_csp` were run instead |
| `cargo audit` / `cargo deny` | **NOT RUN** | tools not installed; advisory DB fetch skipped under the disk limit |
| `programs/staking-suite` (fmt/clippy/test/`build-sbf`) | **NOT RUN** | workspace-excluded; Solana platform tools not installed |
| Live Stripe/Paddle, Vault/KMS/HSM, mainnet RPC, funded trading | **EXTERNAL_REQUIRED** | no credentials, no funded wallet, no provider access |
| External security audit / penetration test | **NOT PERFORMED** | no auditor engaged; no audit claim is made here |
