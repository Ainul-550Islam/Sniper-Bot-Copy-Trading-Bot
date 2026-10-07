#!/usr/bin/env bash
# compare-canonical-to-buyer-source.sh — byte/content compare of the
# canonical source tree against buyer-release/source (PROMPT 5 §L, file 95).
#
# The buyer source tree must be a BYTE-EXACT mirror of the canonical
# product source. This script says exactly where it is not:
#   MISSING  — file exists canonically but not in the buyer tree
#   STALE    — file exists in the buyer tree but not canonically
#   DIFFERS  — both exist, contents differ
#
# Explicit ALLOWED EXCLUSIONS (canonical-side, never part of the product):
#   buyer-release/        the release package itself (this script's target)
#   uploads/              workspace upload staging (session material)
#   PROMPT-4-*.md, PROMPT-5-SPEC.md, FILE-VERIFICATION-REPORT.md
#                         working/session documents for the current build
#                         (PROMPT-2/PROMPT-3 RESULT documents ARE product:
#                         they ship inside the buyer package)
#   dump.rdb              local redis dump (never shipped)
#   generated/dependency dirs (target, node_modules, .next, .git, .turbo,
#   .vercel, dist, build, out, coverage, .arena, .cache, .local,
#   __pycache__, .venv)
#
# Exit codes: 0 = parity, 1 = drift (see report), 2 = environment.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CANONICAL="$ROOT"
BUYER="$ROOT/buyer-release/source"

if [ ! -d "$BUYER" ]; then
  echo "[compare] MISSING buyer source tree: $BUYER" >&2
  exit 2
fi

exec python3 - "$CANONICAL" "$BUYER" <<'PY'
import sys
from pathlib import Path

canonical, buyer = Path(sys.argv[1]), Path(sys.argv[2])

EXCLUDED_DIRS = {
    "target", "node_modules", ".next", ".git", ".turbo", ".vercel", "dist",
    "build", "out", "coverage", ".arena", ".cache", ".local", "__pycache__",
    ".venv", ".mypy_cache", ".pytest_cache", ".ruff_cache",
    "buyer-release", "uploads", ".rustup",
}
# Toolchain binaries installed under <root>/.cargo/bin are local toolchain
# state, never product (a cargo-deny audit.toml in .cargo/ IS product).
EXCLUDED_PREFIXES = [Path(".cargo") / "bin"]
EXCLUDED_FILES = {
    "PROMPT-4-PHASE0-MATRIX.md", "PROMPT-4-POLYMARKET-RESEARCH.md",
    "PROMPT-4-PROGRESS.md", "PROMPT-4-RESULT.md", "PROMPT-5-SPEC.md",
    "FILE-VERIFICATION-REPORT.md", "dump.rdb",
}


def walk(root: Path, apply_exclusions: bool):
    out = set()
    for p in root.rglob("*"):
        if not p.is_file():
            continue
        rel = p.relative_to(root)
        if any(part in EXCLUDED_DIRS for part in rel.parts):
            continue
        if apply_exclusions:
            if any(str(rel).startswith(str(pre)) for pre in EXCLUDED_PREFIXES):
                continue
            if rel.name in EXCLUDED_FILES:
                continue
            # TypeScript emits this local incremental cache during typecheck/build;
            # it is generated state, not buyer source, and must never affect parity.
            if rel.name.endswith(".tsbuildinfo"):
                continue
        else:
            # Buyer side: only prune dependency/build dirs that can never
            # be legitimately present (they are contamination and are
            # reported by verify-release-integrity.sh instead).
            if any(part in ("target", "node_modules", ".next", ".git") for part in rel.parts):
                continue
        out.add(rel)
    return out


canon_files = walk(canonical, apply_exclusions=True)
buyer_files = walk(buyer, apply_exclusions=False)

missing = sorted(canon_files - buyer_files)
stale = sorted(buyer_files - canon_files)
differs = []
same = 0
for rel in sorted(canon_files & buyer_files):
    a = (canonical / rel).read_bytes()
    b = (buyer / rel).read_bytes()
    if a != b:
        differs.append(rel)
    else:
        same += 1

for rel in missing:
    print(f"MISSING  {rel}")
for rel in stale:
    print(f"STALE    {rel}")
for rel in differs:
    print(f"DIFFERS  {rel}")

total = same + len(missing) + len(differs)
print(f"[compare] buyer={buyer}")
print(
    f"[compare] identical={same} missing={len(missing)} "
    f"stale={len(stale)} differs={len(differs)} total_product_files={total}"
)
if missing or stale or differs:
    print("[compare] PARITY FAIL — rebuild with scripts/rebuild-buyer-release.sh")
    sys.exit(1)
print("[compare] PARITY OK — buyer source is a byte-exact mirror of the canonical product")
PY
