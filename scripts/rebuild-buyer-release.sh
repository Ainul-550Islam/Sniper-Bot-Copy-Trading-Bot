#!/usr/bin/env bash
# rebuild-buyer-release.sh — rebuild the buyer package source tree from
# the canonical source (PROMPT 5 §L, file 96).
#
# What this does:
#   1. mirror the canonical product into buyer-release/source with the
#      SAME exclusion list compare-canonical-to-buyer-source.sh enforces
#      (one list, both scripts — no drift between build and check),
#      deleting stale buyer-side files that no longer exist canonically;
#   2. regenerate checksums/ and refresh the release manifest
#      (scripts/update-release-manifest.sh);
#   3. run the parity compare as a post-condition — the rebuild is only
#      successful if the mirror verifies.
#
# The package-level release artifacts (VERSION, docs/, CHANGELOG.md,
# checksums/, evidence/, licenses/, sbom/, manifests/) live OUTSIDE
# buyer-release/source and are preserved; only the source mirror is
# rebuilt. (rsync is not assumed; the mirror is a Python walk with the
# exclusion list, so the script runs anywhere python3 does.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$ROOT/buyer-release/source"

if [ ! -d "$SOURCE" ]; then
  echo "[rebuild] creating buyer source tree: $SOURCE"
  mkdir -p "$SOURCE"
fi

echo "[rebuild] refreshing release manifest (before the mirror, so the mirrored copy is the fresh one)"
"$ROOT/scripts/update-release-manifest.sh"

echo "[rebuild] mirroring canonical -> buyer-release/source"
python3 - "$ROOT" "$SOURCE" <<'PY'
import shutil
import sys
from pathlib import Path

root, dest = Path(sys.argv[1]), Path(sys.argv[2])

EXCLUDED_DIRS = {
    "target", "node_modules", ".next", ".git", ".turbo", ".vercel", "dist",
    "build", "out", "coverage", ".arena", ".cache", ".local", "__pycache__",
    ".venv", ".mypy_cache", ".pytest_cache", ".ruff_cache",
    "buyer-release", "uploads", ".rustup",
}
# Local toolchain state under .cargo/bin is never product.
EXCLUDED_PREFIXES = [Path(".cargo") / "bin"]
EXCLUDED_FILES = {
    "PROMPT-4-PHASE0-MATRIX.md", "PROMPT-4-POLYMARKET-RESEARCH.md",
    "PROMPT-4-PROGRESS.md", "PROMPT-4-RESULT.md", "PROMPT-5-SPEC.md",
    "FILE-VERIFICATION-REPORT.md", "dump.rdb",
}

def product_files():
    for p in sorted(root.rglob("*")):
        if not p.is_file():
            continue
        rel = p.relative_to(root)
        if any(part in EXCLUDED_DIRS for part in rel.parts):
            continue
        if any(str(rel).startswith(str(pre)) for pre in EXCLUDED_PREFIXES):
            continue
        if rel.name in EXCLUDED_FILES:
            continue
        # TypeScript emits this local incremental cache during typecheck/build;
        # it is generated state, not buyer source, and must not enter the mirror.
        if rel.name.endswith(".tsbuildinfo"):
            continue
        yield rel, p

wanted = dict(product_files())

# 1. Delete stale buyer-side files (present there, not canonical product).
stale = 0
for p in dest.rglob("*"):
    if p.is_file() and p.relative_to(dest) not in wanted:
        p.unlink()
        stale += 1
# Prune now-empty directories.
for p in sorted(dest.rglob("*"), reverse=True):
    if p.is_dir() and not any(p.iterdir()):
        p.rmdir()

# 2. Copy new/changed files (byte-exact).
copied = 0
same = 0
for rel, src in wanted.items():
    dst = dest / rel
    if dst.is_file() and dst.read_bytes() == src.read_bytes():
        same += 1
        continue
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    copied += 1

print(f"[rebuild] copied={copied} unchanged={same} stale_removed={stale} total={len(wanted)}")
PY

echo "[rebuild] regenerating source checksums"
mkdir -p "$ROOT/buyer-release/checksums"
( cd "$SOURCE" && find . -type f -print0 | sort -z | xargs -0 sha256sum ) \
  > "$ROOT/buyer-release/checksums/source.sha256"
( cd "$SOURCE" && sha256sum Cargo.lock > "$ROOT/buyer-release/checksums/Cargo.lock.sha256" )

echo "[rebuild] refreshing package-level docs/ from canonical docs/"
python3 - "$ROOT" <<'PYDOC'
import shutil
import sys
from pathlib import Path

root = Path(sys.argv[1])
src = root / "docs"
dst = root / "buyer-release" / "docs"
dst.mkdir(parents=True, exist_ok=True)
wanted = {p.name: p for p in src.glob("*.md")}
stale = 0
for p in dst.glob("*.md"):
    if p.name not in wanted:
        p.unlink()
        stale += 1
copied = same = 0
for name, s in wanted.items():
    d = dst / name
    if d.is_file() and d.read_bytes() == s.read_bytes():
        same += 1
        continue
    shutil.copyfile(s, d)
    copied += 1
print(f"[rebuild] package docs: copied={copied} unchanged={same} stale_removed={stale} total={len(wanted)}")
PYDOC

echo "[rebuild] regenerating package checksums (SOURCE-TREE, SHA256SUMS, all-files)"
PKG="$ROOT/buyer-release"
# SOURCE-TREE.sha256 — the documented method, recomputed fail-closed.
N_FILES="$(cd "$SOURCE" && find . -type f | wc -l | tr -d ' ')"
DIGEST="$(cd "$SOURCE" && find . -type f | sort | xargs sha256sum | sha256sum | awk '{print $1}')"
{
  echo "# source-tree digest of the delivered source/ directory"
  echo "# method: cd <PKG>/source && find . -type f | sort | xargs sha256sum | sha256sum"
  echo "# files: $N_FILES"
  echo "$DIGEST  source/"
} > "$PKG/checksums/SOURCE-TREE.sha256"
# SHA256SUMS + all-files.sha256 — every file in the package except the
# checksum directory itself (self-referential entries are forbidden).
( cd "$PKG" && find . -type f -not -path "./checksums/*" -print0 | sort -z | xargs -0 sha256sum ) \
  | sed 's|^\./||' > "$PKG/checksums/all-files.sha256"
{
  echo "# SHA256 checksums for buyer-release artifacts — computed independently per file"
  echo "# generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# format: '<sha256>  <relpath>' — verify with: cd <PKG> && sha256sum -c checksums/SHA256SUMS"
  cat "$PKG/checksums/all-files.sha256"
} > "$PKG/checksums/SHA256SUMS"

echo "[rebuild] verifying parity post-condition"
"$ROOT/scripts/compare-canonical-to-buyer-source.sh"

echo "[rebuild] DONE — buyer source mirror rebuilt and verified"
