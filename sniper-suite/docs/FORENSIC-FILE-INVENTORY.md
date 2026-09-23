# Forensic file inventory

This document restores the forensic-inventory path referenced by
`CHANGELOG.md`, `docs/SOURCE-OF-TRUTH.md`, and `docs/REPOSITORY-MAP.md`.
The original workspace-wide report covered vendor packaging, evidence copies,
and generated artifacts outside the canonical source tree. Those copies are
not application source and are intentionally not reproduced here.

## Canonical source boundary

The only editable application source is the repository's `sniper-suite/`
directory. In that tree:

- `crates/` is the seven-crate Rust application workspace;
- `programs/staking-suite/` is the separately locked Solana program;
- `crates/core/migrations/` is the ordered PostgreSQL migration set;
- `scripts/`, `.github/`, root configuration, and `docs/` are release and
  operational material;
- `target/`, package mirrors, archives, database dumps, logs, keypairs, and
  evidence copies are never source.

The source-of-truth policy is defined in `SOURCE-OF-TRUTH.md`. The annotated
canonical layout is maintained in `REPOSITORY-MAP.md`.

## Reproducible current inventory

Run from `sniper-suite/`:

```bash
find . -path './.git' -prune -o -path './target' -prune \
  -o -path './programs/staking-suite/target' -prune -o -type f -print \
  | LC_ALL=C sort

git ls-files | LC_ALL=C sort

git ls-files -s | LC_ALL=C sort
```

The first command inventories the received tree without generated build
outputs. The second is authoritative when Git metadata is present. The third
also records executable bits, which matter for shell scripts.

## Classification rules

| Class | Paths | Treatment |
|---|---|---|
| Application source | `crates/**/*.rs`, workspace manifests and lockfile | Canonical; build and test |
| On-chain source | `programs/staking-suite/**` except `target/` | Canonical; separate build and test |
| Database schema | `crates/core/migrations/*.sql` | Canonical; forward-only and contiguous |
| Runtime/deployment | root TOML/templates, Docker assets, `.github/`, `scripts/` | Canonical; syntax and release-gate checked |
| Documentation | root Markdown and `docs/*.md` | Canonical; links and claims checked |
| Generated output | every `target/`, `build/`, coverage directory | Exclude |
| Secrets/state | `.env`, keypairs, dumps, Redis state, populated data | Exclude |
| Distribution mirrors | buyer-package `source-tree/`, tarballs, copied docs | Derived; never edit as source |
| Evidence | logs, reports, benchmark captures, binaries | Historical/derived; never substitute for source |

## Duplicate handling

A byte-identical file in a delivery mirror or archive is a derived copy, not a
second source of truth. Edit the canonical path first, regenerate the package,
and compare hashes. Never merge a stale package copy back over a newer source
file merely because its name matches.

## Completeness checks

`scripts/verify-delivery.sh` enforces required files, migration continuity,
version identity, documentation count, hygiene, Markdown links, and invisible
Unicode checks. `scripts/release-check.sh` adds formatting, compilation,
lints, tests, and supply-chain checks.

This restored inventory intentionally contains no unverifiable historical
per-file hashes or stale file totals. Recompute them from the received tree
with the commands above.
