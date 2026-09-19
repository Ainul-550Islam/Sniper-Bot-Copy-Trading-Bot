# Release engineering

How this repository is versioned, built reproducibly, validated and cut for
release. Everything below describes the actual mechanism in this tree — no
aspirational process.

## 1. Versioning

**Single source of truth:** `[workspace.package].version` in the root
`Cargo.toml`. All seven workspace crates inherit it
(`version.workspace = true`); the standalone staking program
(`programs/staking-suite/Cargo.toml`) carries the same version explicitly
(it is excluded from the workspace by design — different toolchain for
`cargo build-sbf`).

Mirrors (kept consistent mechanically, never by hand alone):

| Mirror | Where | Enforced by |
|---|---|---|
| `VERSION` file | repo root (human/script-readable) | `scripts/release-check.sh` fails on mismatch |
| Runtime `version` | `GET /health` body and `bot_build_info{version=…}` metric | compiled-in `CARGO_PKG_VERSION` (server crate) |
| `CHANGELOG.md` | `[x.y.z]` heading | release checklist below |

**Strategy:** SemVer against the control-plane HTTP/WS contract and the
configuration file schema. Breaking changes to either require a major (or,
pre-1.0, a minor) bump plus a CHANGELOG entry. The database schema versions
independently and **forward-only** (see §4).

## 2. Release manifest (what is exposed where)

`release-manifest.json` (repo root) is the machine-readable delivery
manifest: version, component list, migration high-water mark, toolchain
pins, executed test counts, verification-status taxonomy and the external
handover blockers. It contains **no build timestamp** (reproducibility) and
**no commit hash** (the file is part of the commit it would describe — the
authoritative commit/tag lives in git history and the release notes).
`scripts/release-check.sh` fails the release if the file is missing or its
`version` disagrees with `VERSION`/`Cargo.toml`.

Runtime build metadata is additionally exposed from values that are
themselves deterministic:

| Field | Source |
|---|---|
| Project version | `GET /health` → `version`; `bot_build_info{version}` |
| Schema/migration version | `GET /api/db` → applied migration count; `_sqlx_migrations` table lists every applied version + checksum |
| Rust toolchain | `rust-toolchain.toml` (pin), `cargo --version` at build time |
| Git commit | `git rev-parse HEAD` of the checkout the release was cut from (recorded in the release notes/tag — the binary does not embed it, to stay reproducible) |
| Enabled components | `GET /api/modules` + effective config via `GET /api/config` (redacted) |
| Dependency provenance | committed `Cargo.lock` (app) and `programs/staking-suite/Cargo.lock` (program) |

No secret, credential, or full internal filesystem path is exposed by any of
these surfaces (`/api/config` is redacted by construction; health `detail`
strings are booleans/counts/enum names only).

## 3. Reproducible build

Verified properties of this tree (inspected, not assumed):

- `Cargo.lock` committed for **both** cargo projects → exact dependency set.
- `resolver = "2"`; no `[patch]` sections; no git/path dependencies outside
  the repository.
- **Zero `build.rs` scripts** in any workspace crate or the staking program →
  no environment-dependent code generation.
- No build timestamp, git hash, or hostname is compiled into any binary
  (`env!("CARGO_PKG_VERSION")` is the only compile-time environment value).
- `sqlx` is used **without** compile-time database access: all queries are
  runtime-checked strings; the only compile-time embedding is
  `sqlx::migrate!()` reading `crates/core/migrations/*.sql` from the tree —
  deterministic, and no `DATABASE_URL` is needed to build.
- Toolchain pinned by `rust-toolchain.toml` (1.98.1 + rustfmt + clippy);
  the Docker builder image (`rust:1.98.1-bookworm`), the CI app job (pin file
  governs) and the CI program job (`dtolnay/rust-toolchain@1.98.1`) all use
  the same version. `scripts/release-check.sh` fails if these drift.
- Release profile is fixed in the root `Cargo.toml` (`opt-level=3`,
  `lto="thin"`, `codegen-units=1`, `panic="unwind"`, `strip=true`); an
  additional `snipe` profile (`lto="fat"`) exists for latency-sensitive
  builds.

**Honest limits of reproducibility:** bit-for-bit identical binaries
additionally require the same rustc patch version (pinned), the same target
triple, and the same versions of system link libraries (OpenSSL is *not*
linked — TLS is rustls; `libudev` is a dynamic system dependency of the
Solana client stack). Source-level reproducibility (same sources + lockfiles
+ pinned toolchain ⇒ same dependency graph and semantics) is what this tree
guarantees; byte-identical artifact attestation (e.g. via `cargo
build --build-plan` hashing or rebuilderd) is **not** set up and is not
claimed.

## 4. Database migrations

- Location: `crates/core/migrations/0001..0011_*.sql`, embedded into the
  binary by `sqlx::migrate!`.
- **Forward-only.** No down migrations exist and none should be invented:
  several migrations are irreversible by nature (data columns, CHECK
  supersets, claim tables). The recovery path from a bad schema change is
  restore-from-backup + roll-forward (see `docs/BACKUP-RESTORE.md`).
- Ordering: zero-padded monotonic prefixes; sqlx applies unapplied versions
  in order inside `_sqlx_migrations` and records a checksum per file. An
  edited-after-apply migration fails startup loudly (checksum mismatch) —
  never edit an applied migration; add a new one.
- Startup behavior: `[database].auto_migrate` (default `true`) applies
  pending migrations at connect. Duplicate application is impossible
  (tracked per version). `GET /api/db` reports the applied count.
- Older binaries vs newer schema: migrations are additive, so a pinned
  rollback image generally runs against a newer schema — verify per release
  note before rolling back (also stated in `docs/OPERATIONS.md`).

## 5. SBOM / dependency provenance

- `cargo deny check` (policy in `deny.toml`) gates advisories, duplicate
  bans, license allow-list and sources — in CI and in `release-check.sh`.
- `cargo audit` runs against **both** lockfiles.
- SBOM generation is deliberately **not** vendored: no SBOM tool is
  installed in this environment, and adding one just for appearance was
  rejected. When the releasing team wants an SBOM, the reproducible inputs
  are the committed lockfiles; generate with e.g.
  `cargo cyclonedx --lockfile Cargo.lock --all` (or `cargo sbom`) at release
  time. Status here: **NOT EXECUTED — tooling not available in the build
  sandbox**; the lockfiles themselves are the authoritative dependency
  record.

## 6. Cutting a release (checklist)

1. Work is merged; `CHANGELOG.md` has the entry for the new version.
2. Bump `[workspace.package].version` (and the staking crate's `version`),
   update `VERSION`, run `cargo check` once so `Cargo.lock` records the new
   crate versions.
3. Run `./scripts/release-check.sh` — it must exit 0. Provide
   `POSTGRES_URL`/`REDIS_URL` to execute (not skip) the gated integration
   suites.
4. Tag and push: CI runs the full matrix (fmt, clippy `-D warnings`, build,
   workspace tests against real Postgres+Redis service containers, staking
   fmt/clippy/host tests/`build-sbf`/validator e2e, cargo-audit ×2,
   cargo-deny hard gates, docker image build + `/api/health` smoke test).
5. Record in the release notes: tag/commit, toolchain (1.98.1 or the new
   pin), migration high-water mark (e.g. `0011`), and any schema/config
   contract changes.
6. Docker image: built by the CI `docker` job; retag/push per your registry
   policy (CI never pushes — no registry credentials by design).
