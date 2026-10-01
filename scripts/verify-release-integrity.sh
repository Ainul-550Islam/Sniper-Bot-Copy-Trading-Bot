#!/usr/bin/env bash
# verify-release-integrity.sh — the combined release gate (PROMPT 5 §L,
# file 98).
#
# Four checks, all must pass:
#   1. SOURCE PARITY   — buyer-release/source is a byte-exact mirror of
#                        the canonical product (compare-canonical…sh)
#   2. MANIFEST        — the manifest's counts match the real trees
#   3. CONTAMINATION   — the buyer package carries no secrets, no
#                        key/wallet material, no dumps, no build
#                        outputs, no binaries
#   4. VERSION         — VERSION == Cargo.toml == manifest == migration
#                        high-water expectations
#
# Exit 0 only when everything passes; the report says exactly what failed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/buyer-release"
FAIL=0

echo "== [1/4] source parity =="
if ! "$ROOT/scripts/compare-canonical-to-buyer-source.sh"; then
  echo "== [1/4] FAIL"
  FAIL=1
else
  echo "== [1/4] OK"
fi

echo "== [2/4] manifest counts =="
python3 - "$ROOT" <<'PY' || FAIL=1
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
manifest = json.loads((root / "release-manifest.json").read_text())

# Same convention as the manifest and the batch6 guard test:
# rust_files counts .rs under crates/ ONLY (programs/ is its own component).
rust = sum(1 for _ in (root / "crates").rglob("*.rs"))
migrations = list((root / "crates/core/migrations").glob("*.sql"))
ok = True
if manifest.get("rust_files") != rust:
    print(f"  manifest rust_files={manifest.get('rust_files')} but actual={rust}")
    ok = False
if manifest.get("migrations") != len(migrations):
    print(f"  manifest migrations={manifest.get('migrations')} but actual={len(migrations)}")
    ok = False
hw = manifest.get("components", {}).get("database_migrations", {}).get("high_water_mark")
latest = max((m.name for m in migrations), default="")
if hw and latest and not latest.startswith(hw):
    print(f"  manifest high_water_mark={hw} but latest migration is {latest}")
    ok = False
print("  manifest counts verified" if ok else "  MANIFEST DRIFT")
sys.exit(0 if ok else 1)
PY
[ "$FAIL" -eq 0 ] && echo "== [2/4] OK" || echo "== [2/4] FAIL"

echo "== [3/4] contamination =="
CONTAM=0
# Banned directories anywhere in the package.
while IFS= read -r -d '' d; do
  echo "  CONTAMINATED directory: ${d#"$PKG"/}"
  CONTAM=1
done < <(find "$PKG" -type d \( -name target -o -name node_modules -o -name .next -o -name .turbo -o -name .vercel -o -name dist -o -name build -o -name coverage -o -name .git \) -print0)

# Banned secret/key/wallet/dump files. .env.template is the DOCUMENTED
# exception (a placeholder file with no values). Source files whose names
# merely contain "credentials" (e.g. credentials.rs) are code, not data.
while IFS= read -r -d '' f; do
  base="$(basename "$f")"
  case "$base" in
    .env|*.env|.env.local|.env.production|.env.*.local)
      echo "  CONTAMINATED env file: ${f#"$PKG"/}"; CONTAM=1 ;;
    *.pem|*.key|*.p12|*.pfx)
      echo "  CONTAMINATED key material: ${f#"$PKG"/}"; CONTAM=1 ;;
    id.json|id_*.json|*wallet*.json)
      echo "  CONTAMINATED wallet file: ${f#"$PKG"/}"; CONTAM=1 ;;
    *.rdb|*.dump|*.dmp|*.sql.gz)
      echo "  CONTAMINATED dump: ${f#"$PKG"/}"; CONTAM=1 ;;
  esac
done < <(find "$PKG" -type f -print0)

# Binaries: ELF/Mach-O/PE anywhere in the package.
while IFS= read -r -d '' f; do
  if head -c 4 "$f" | od -An -tx1 | grep -qi "7f 45 4c 46\|cf fa ed fe\|4d 5a"; then
    echo "  CONTAMINATED binary: ${f#"$PKG"/}"
    CONTAM=1
  fi
done < <(find "$PKG" -type f -print0)

if [ "$CONTAM" -ne 0 ]; then echo "== [3/4] FAIL"; FAIL=1; else echo "== [3/4] OK"; fi

echo "== [4/4] version consistency =="
VER="$(tr -d '[:space:]' < "$ROOT/VERSION")"
CARGO_VER="$(grep -E '^version =' "$ROOT/Cargo.toml" | head -n1 | sed 's/.*"\(.*\)"/\1/')"
MAN_VER="$(python3 -c "import json;print(json.load(open('$ROOT/release-manifest.json')).get('version',''))")"
BUYER_VER="$(tr -d '[:space:]' < "$PKG/VERSION" 2>/dev/null || echo '')"
VER_OK=1
[ "$VER" = "$CARGO_VER" ] || { echo "  VERSION($VER) != Cargo.toml($CARGO_VER)"; VER_OK=0; }
[ "$VER" = "$MAN_VER" ] || { echo "  VERSION($VER) != manifest($MAN_VER)"; VER_OK=0; }
[ -z "$BUYER_VER" ] || [ "$VER" = "$BUYER_VER" ] || { echo "  VERSION($VER) != buyer VERSION($BUYER_VER)"; VER_OK=0; }
if [ "$VER_OK" -ne 1 ]; then echo "== [4/4] FAIL"; FAIL=1; else echo "== [4/4] OK ($VER)"; fi

echo
if [ "$FAIL" -ne 0 ]; then
  echo "RESULT: RELEASE INTEGRITY FAILED"
  exit 1
fi
echo "RESULT: RELEASE INTEGRITY OK — parity, manifest, contamination, version all verified"
exit 0
