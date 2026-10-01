#!/usr/bin/env bash
# update-release-manifest.sh — regenerate the release manifest's
# measurable fields from the actual trees (PROMPT 5 §L, file 97).
#
# Refreshed from reality (never hand-edited):
#   * rust_files / docs_files — real counts over the canonical product
#   * components.database_migrations.count + high_water_mark — from
#     crates/core/migrations
#   * verification.manifest — sha256 of every tracked file set, so the
#     manifest can prove WHICH tree it measured
#
# Everything else (version, notes, evidence) stays as-is; this script
# refuses to bump or invent anything it cannot count.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/release-manifest.json"
BUYER_MANIFEST="$ROOT/buyer-release/manifests/release-manifest.json"

if [ ! -f "$MANIFEST" ]; then
  echo "[manifest] missing $MANIFEST" >&2
  exit 2
fi

python3 - "$MANIFEST" "$BUYER_MANIFEST" "$ROOT" <<'PY'
import hashlib
import json
import re
import sys
from pathlib import Path

manifest_path, buyer_manifest_path, root = sys.argv[1], sys.argv[2], Path(sys.argv[3])
manifest = json.loads(Path(manifest_path).read_text())

EXCLUDED_DIRS = {
    "target", "node_modules", ".next", ".git", ".turbo", ".vercel", "dist",
    "build", "out", "coverage", ".arena", ".cache", ".local", "__pycache__",
    ".venv", ".mypy_cache", ".pytest_cache", ".ruff_cache", "buyer-release",
    "uploads", ".rustup",
}
# Local toolchain state under .cargo/bin is never product.
EXCLUDED_PREFIXES = [".cargo/bin"]
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
        if any(str(rel).startswith(pre) for pre in EXCLUDED_PREFIXES):
            continue
        if rel.name in EXCLUDED_FILES or rel.name in {"release-manifest.json"} and rel.parent == root:
            continue
        if rel.name == "release-manifest.json" and len(rel.parts) == 1:
            continue
        yield rel

files = list(product_files())
# `rust_files` follows the established manifest/test contract: .rs files
# under crates/ ONLY (programs/ has its own lockfile and is counted as a
# component, not as workspace rust source).
rust_files = [f for f in files if f.suffix == ".rs" and f.parts[0] == "crates"]
docs_files = [f for f in files if f.parts[0] == "docs"]

migrations_dir = root / "crates" / "core" / "migrations"
migrations = sorted(p.name for p in migrations_dir.glob("*.sql")) if migrations_dir.is_dir() else []
high_water = ""
hw_match = re.match(r"(\d+)", migrations[-1]) if migrations else None
if hw_match:
    high_water = hw_match.group(1)

manifest["rust_files"] = len(rust_files)
manifest["docs_files"] = len(docs_files)
manifest["migrations"] = len(migrations)
# test_count follows the final-release-check contract: #[test] (non-tokio)
# attribute lines under crates/ — recomputed here so it can never go stale.
test_count = 0
for f in files:
    if f.suffix == ".rs" and f.parts[0] == "crates":
        test_count += f.read_text(errors="replace").count("#[test]")
manifest["test_count"] = test_count
# components.docs_count follows the same rule as docs_files (verify-delivery
# reads this key) — recomputed so it can never drift from the tree.
if isinstance(manifest.get("components"), dict) and "docs_count" in manifest["components"]:
    manifest["components"]["docs_count"] = len(docs_files)
components = manifest.setdefault("components", {})
db = components.setdefault("database_migrations", {})
db["count"] = len(migrations)
db["high_water_mark"] = high_water

# A short, verifiable fingerprint of what was measured: counts + a
# rolling digest over relative paths (not contents — checksums/ holds
# the content hashes).
h = hashlib.sha256()
for f in files:
    h.update(str(f).encode())
    h.update(b"\0")
manifest["verification"]["manifest"] = {
    "product_files": len(files),
    "rust_files": len(rust_files),
    "docs_files": len(docs_files),
    "tree_digest": h.hexdigest(),
    "generated_by": "scripts/update-release-manifest.sh",
}

Path(manifest_path).write_text(json.dumps(manifest, indent=2) + "\n")
# The buyer package carries its own copy of the manifest.
if Path(buyer_manifest_path).exists():
    Path(buyer_manifest_path).write_text(json.dumps(manifest, indent=2) + "\n")

print(
    f"[manifest] product_files={len(files)} rust_files={len(rust_files)} "
    f"docs_files={len(docs_files)} migrations={len(migrations)} (high water {high_water or '—'})"
)
PY

echo "[manifest] updated $MANIFEST"
