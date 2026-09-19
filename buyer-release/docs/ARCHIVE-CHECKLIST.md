# Final delivery archive checklist

Defines exactly what the seller's final archive (git bundle + source
snapshot) must contain, and what must never be in it. **The archive itself
is NOT produced in the packaging sandbox**: the sandbox lost its `.git`
metadata in an infrastructure re-provision, and an archive built here could
not carry the authoritative history. The real archive MUST be produced from
the seller's authoritative repository containing `9c677cd` → `0e139c3` (plus
the documentation-pass commits), so that history, tags and hashes survive
intact. No fake history may be substituted.

## How to produce the archive (seller side, authoritative repo)

```bash
# 1. Confirm state:
git log --oneline          # contains 9c677cd (release) and 0e139c3 (freeze)
git status --short         # clean
./scripts/verify-delivery.sh   # bundle integrity (docs, versions, hygiene)

# 2. Full-history bundle (primary artifact — preserves real history):
git bundle create sniper-suite-0.1.0.bundle --all
git bundle verify sniper-suite-0.1.0.bundle

# 3. Optional convenience snapshot (no history; secondary artifact only):
git archive --format=tar.gz --prefix=sniper-suite-0.1.0/ -o sniper-suite-0.1.0-src.tar.gz HEAD

# 4. Record identities for the transfer paperwork:
git rev-parse HEAD
sha256sum sniper-suite-0.1.0.bundle sniper-suite-0.1.0-src.tar.gz
```

The buyer verifies per `docs/BUYER-QUICKSTART.md` §1 (clone from bundle →
`git log` shows `0e139c3` on `9c677cd` → `verify-delivery.sh` green).

## INCLUDE (must be present — all are tracked files)

- [ ] **Source** — `crates/` (7 crates: 71 src `.rs` + 14 test `.rs` +
      7 Cargo.tomls) and `programs/staking-suite/` (5 src + 1 test +
      Cargo.toml + `.cargo/` configs).
- [ ] **Lockfiles** — root `Cargo.lock` (706 packages) **and**
      `programs/staking-suite/Cargo.lock` (580 packages). Both are
      mandatory: they are the authoritative dependency record and the
      reproducibility basis.
- [ ] **Migrations** — `crates/core/migrations/0001`–`0011` (all 11 `.sql`).
- [ ] **Docs** — all 36 files under `docs/` (13 engineering + 14 buyer
      package + 9 final delivery), plus root `README.md`, `CHANGELOG.md`,
      `AUDIT.md` (evidence trail — never strip it), `SECURITY.md`.
- [ ] **Config examples** — `config.toml.example`, `.env.template`,
      `rust-toolchain.toml`, `deny.toml`, root `.cargo/audit.toml`.
- [ ] **Deployment** — `Dockerfile`, `docker-compose.yml`, `.dockerignore`,
      `.github/workflows/ci.yml`.
- [ ] **CI** — the workflow above (part of deployment).
- [ ] **Release assets** — `release-manifest.json`, `VERSION`, `LICENSE`,
      `scripts/release-check.sh`, `scripts/verify-delivery.sh`.
- [ ] **Git history** — via the bundle (`--all`): both release commits and
      every documentation-pass commit.

## EXCLUDE (must never be in the archive)

- [ ] `target/` (any Rust build output — root, crates, or program).
- [ ] `build/` or any scratch/build directories.
- [ ] Local database files (Postgres data dirs, dumps made during testing —
      `*.dump`, `*.sql.gz` snapshots of live data).
- [ ] Redis data (`dump.rdb`, `appendonly.aof`).
- [ ] `.env` (the template `.env.template` IS included; a filled `.env` is
      a secret file).
- [ ] Secret files of any kind: `*.pem`, `id_*`, `*keypair*.json`,
      `*-keypair.json`, wallet files, token files, credential stores.
- [ ] Generated logs (`*.log`), profiling/flamegraph outputs.
- [ ] Private credentials or provider API keys in any format.
- [ ] Temporary installer files (rustup-init, toolchain tarballs, PG/Redis
      source archives).
- [ ] Editor/OS junk (`.DS_Store`, `*.swp`, `Thumbs.db`, `.idea/`, `.vscode/`
      with local settings).

The repository's `.gitignore` already excludes all of the above from
tracking, and `scripts/release-check.sh` (secret + marker scans) and
`scripts/verify-delivery.sh` (hygiene checks) fail if any of it appears in
the tree. A `git archive`/`git bundle` of a clean tree therefore satisfies
the EXCLUDE list by construction — verify anyway with:

```bash
tar -tzf sniper-suite-0.1.0-src.tar.gz | grep -E 'target/|\.env$|\.log$|keypair|\.dump$|dump\.rdb' && echo "CONTAMINATED" || echo "clean"
```

## Integrity requirements

1. The bundle must `git bundle verify` cleanly and contain `9c677cd` and
   `0e139c3` as reachable commits.
2. `git status` in the buyer's clone must be clean after checkout.
3. `./scripts/verify-delivery.sh` must pass in the checked-out tree.
4. SHA-256 sums of both artifacts recorded in the transfer paperwork
   (this checklist intentionally contains no hashes — they are produced at
   packaging time on the authoritative machine).
5. Version identity: `VERSION` = `Cargo.toml` = `release-manifest.json` =
   `0.1.0`.

## What is explicitly NOT part of the archive

- Deployed on-chain program (nothing is deployed; `declare_id!` is a
  placeholder).
- Any live/production database content, keys, tokens, or customer data
  (none exist).
- Docker images (buyer builds from the Dockerfile).
- CI run history (buyer's runners execute the workflow after transfer).
- External-audit reports (none exist — `docs/BUYER-RISK-REGISTER.md` #1).
