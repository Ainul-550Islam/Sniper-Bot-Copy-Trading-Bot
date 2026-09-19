# Source provenance — how to verify and reproduce every hash

Companion to `SOURCE-PROVENANCE.json` (machine-readable, generated
2026-09-19). This file explains what each hash covers and gives exact
commands a buyer can run on a clean machine to reproduce them.

## 1. What is covered

| Hash | Covers | Where |
|---|---|---|
| `tree_sha256` | all 183 repo files (content + path + size + line count), `target/` excluded | JSON `.tree` |
| per-file `sha256` + `executable` | each individual file, incl. exec bit | JSON `.files[]` |
| `source_archive.sha256` | normalized tarball of the full source tree | `source/sniper-suite-FINAL-src.tar.gz` |
| `staking_program_so.sha256` | deterministic staking `.so` | `binaries/staking_suite.so` |
| `final_manifest.sha256`, `package_readme.sha256` | package root documents | package root |
| `lockfiles.*` | both `Cargo.lock` files (dependency graph identity) | source tree |
| `source_inventory.*` | the evidence inventory pair | `evidence/inventory/` |

Checksums `SHA256SUMS{,.json}` are generated **after** this provenance pair
and cover it — self-reference is avoided by design, so this file cannot
contain their hashes; verify them with `sha256sum -c` instead.

## 2. Reproduce per-file hashes and the tree hash

From the extracted `source-tree/` (or the tarball extraction):

```bash
# per-file hashes (compare against JSON .files[])
find . -type f -not -path './target/*' | sort | xargs sha256sum

# tree hash — exact method recorded in JSON .tree.hash_method:
# sha256 over records "path\0bytes\0lines\0file_sha256\0", sorted paths,
# no ./ prefix. Reference implementation:
python3 - <<'PY'
import hashlib, os, subprocess
files = sorted(f[2:] for f in subprocess.check_output(
    ['find','.','-type','f','-not','-path','./target/*'], text=True).split())
h = hashlib.sha256(); n = b = l = 0
for f in files:
    fb = os.path.getsize(f); fl = sum(1 for _ in open(f,'rb'))
    fs = hashlib.sha256(open(f,'rb').read()).hexdigest()
    n += 1; b += fb; l += fl
    h.update(f"{f}\x00{fb}\x00{fl}\x00{fs}\x00".encode())
print(n, b, l, h.hexdigest())
PY
```

Expected: `183 3293320 86576` and
`033b582f79c5e1d24a301895fa98d37fae1ed01de15f0052a4b2777a6dd06946`.

## 3. Reproduce the archive hash (normalized tarball)

```bash
cd source-tree
tar --sort=name --mtime=@1758240000 --owner=0 --group=0 --numeric-owner \
    --format=gnu -cf - . | gzip -9n > /tmp/rebuilt.tar.gz
sha256sum /tmp/rebuilt.tar.gz   # must equal source_archive.sha256
```

Requires GNU tar ≥ 1.28 and gzip. The vendor verified
rebuild-from-extraction is byte-identical (double extraction, 0 diffs).

## 4. Reproduce the `.so` hash (full cold rebuild)

Follow `docs/BUYER-REPRODUCTION-GUIDE.md` steps 1–6 (rustup 1.98.1, agave
2.1.21 tarball with the recorded sha256, platform-tools v1.43 fetched by
`cargo build-sbf`), then:

```bash
cd programs/staking-suite && cargo build-sbf
sha256sum target/deploy/staking_suite.so
# expected: 57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564
```

The vendor proved this hash byte-identical across a first build, an
incremental rebuild, and a from-scratch cold rebuild on a re-provisioned
machine (`evidence/compile-logs/phase3-sbf-determinism-rerun.log`). Note:
bit-identical host binaries additionally require matching system library
versions — documented honestly in `docs/RELEASE.md` §reproducibility.

## 5. Environment of generation

Debian-based x86_64 sandbox, kernel/arch as recorded in JSON
`.build_environment`, 2 CPUs / 2 GB RAM / no swap. This pass made **zero
Rust source changes** (one shell-script defect fix + documentation), so no
compiler was required at provenance time; all test evidence was recorded
earlier on byte-identical Rust sources (see `BUYER-FINAL-RELEASE-MANIFEST.json`
`.test_counts.note`).

## 6. Secrets

None. The tree was scanned with the release-check secret-pattern gate
families plus manual review; `.env.template` carries variable names only;
key material is buyer-supplied at deployment time and never stored in the
repo, evidence, or this package.
