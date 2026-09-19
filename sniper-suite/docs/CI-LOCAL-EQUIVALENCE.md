# CI ↔ local-execution equivalence map

`.github/workflows/ci.yml` defines 4 jobs. No GitHub runner was available in
the delivery/hardening sandboxes, so **no actual GitHub Actions run exists** —
this file maps every CI gate to the local execution that covers the same
logic, with its evidence. A local execution is NOT a GitHub Actions run; the
remaining runner-specific actions are listed at the bottom.

Toolchain pin consistency (checked by `scripts/release-check.sh` gate
"toolchain pin consistency"):

| Location | Pin |
|---|---|
| `rust-toolchain.toml` | channel `1.98.1` + rustfmt + clippy |
| `Dockerfile` | `rust:1.98.1-bookworm` |
| `ci.yml` program job | `dtolnay/rust-toolchain@1.98.1` (explicit; the job's working-directory has no rust-toolchain.toml of its own) |
| `ci.yml` app job | `dtolnay/rust-toolchain@stable` + repo `rust-toolchain.toml` overrides to 1.98.1 at invocation time |
| `ci.yml` program job Solana tools | `solana-foundation/actions/install-solana@v1` with `solana_version: '2.1.21'` |
| Local hardening-pass toolchain | rustc 1.98.1 (48a229cea 2026-09-01), cargo 1.98.1 (797e8a9bc), agave/solana-cli 2.1.21 (src:8a085eeb), platform-tools v1.43 (sbf rustc 1.79.0) |

No version drift: the local hardening pass ran the exact versions CI pins.

## Job: app (fmt / clippy / build / test + compose config)

| CI step | CI command | Local equivalent executed | Result / evidence |
|---|---|---|---|
| rustfmt | `cargo fmt --all --check` | same command (hardening pass + release-check gate) | exit 0 — `evidence/phase2-fmt.log`, release-check log |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | same + the STRICTER `--all-features -D warnings` | exit 0 both — release-check log; prebuild AF clippy log |
| build | `cargo build --workspace --all-targets` | `cargo check --workspace --all-targets` (exit 0) + all test/bench binaries fully built and executed (stronger than check for test targets); the server bin target was built and RUN in the phase-8 startup evidence | `evidence/phase2-check.log`, `phase8b-*` |
| test (PG16 + Redis7 services) | `cargo test --workspace -- --test-threads=1` with POSTGRES_URL/REDIS_URL | same command against real PostgreSQL **17.11** + Redis **8.0.2** (newer than CI's service images) | 537/537 exit 0 — `evidence/phase2-test-workspace.log`; also `--all-features` 537/537 (`phase2-test-workspace-allfeat.log`) |
| compose config | `docker compose config -q` | **BLOCKED** (no docker CLI/daemon in sandbox) | compose file statically verified in prior passes; buyer action below |

## Job: program (staking fmt / clippy / test / build-sbf / validator e2e)

| CI step | CI command | Local equivalent executed | Result / evidence |
|---|---|---|---|
| rustfmt | `cargo fmt --check` | same | exit 0 — hardening pass log |
| clippy | `cargo clippy --all-targets -- -D warnings` | same | exit 0 — `evidence/phase2-staking-clippy.log` |
| unit tests (host) | `cargo test` | same | 71/71 (+3 gated e2e compile) |
| install Solana tools | `solana_version: '2.1.21'` | official agave v2.1.21 release tarball (SHA-256 `5da3359e…`) | `solana --version` = 2.1.21 |
| build-sbf | `cargo build-sbf` | same (platform-tools v1.43) | 187,504-byte .so, SHA-256 `57a890fa…`; byte-identical rebuild |
| validator e2e | `STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | EXACT same command | **3/3 passed, 160.72 s** — `evidence/phase5-full-batch.log` |

## Job: security (cargo-audit ×2, cargo-deny ×2)

| CI step | Local equivalent | Result / evidence |
|---|---|---|
| `cargo audit` (app lockfile) | same, cargo-audit 0.22.2 | 0 errors, 9 allow-listed warnings (`.cargo/audit.toml`) — release-check log |
| `cargo audit` (program lockfile) | same | 0 errors — release-check log |
| `cargo deny check advisories bans sources` + `licenses` | `cargo deny check` (all four), cargo-deny 0.18.9 | advisories/bans/licenses/sources ok — release-check log |

## Job: docker (image build + container smoke)

| CI step | Local equivalent | Status |
|---|---|---|
| `docker/build-push-action` (build, load, no push) | none — no Docker daemon in sandbox | **BLOCKED** (buyer action) |
| smoke: container serves `/api/health` in paper mode with NO DB/Redis/keys | native-equivalent: the debug server binary was built and started with the example config against a restored PostgreSQL; `/health`, `/ready`, `/api/health`, `/api/status` (paper, live_allowed=false), `/api/audit/verify`, `/metrics` all served; clean SIGTERM shutdown | **PASS (native equivalent, labeled — not a container run)** — `evidence/phase8b-*` |

## Remaining GitHub-runner-specific actions (buyer)

1. Push the tree to the real repository; confirm all 4 CI jobs go green on a
   GitHub runner (the exact commands above; no drift expected — same pins).
2. `docker compose config -q` gate (app job) and the docker job image build +
   container smoke on the runner's Docker.
3. Record the Actions run URL as evidence; until then, cite this file plus
   the local logs — never label local runs as GitHub Actions runs.
