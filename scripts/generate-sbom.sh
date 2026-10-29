#!/usr/bin/env bash
# generate-sbom.sh — emit the repository's ONE canonical CycloneDX SBOM.
#
# `sbom.json` is the only SBOM in this repository, and it is the CycloneDX
# document (`bomFormat: "CycloneDX"`). A second, byte-identical copy used to be
# written to `sbom.cyclonedx.json`; two copies of the same bill of materials
# drift apart the first time only one of them is regenerated, and a buyer
# reading the data room cannot tell which copy was audited. Emitting one file
# removes that ambiguity for good.
#
# Usage: scripts/generate-sbom.sh [output-path]   (default: <repo>/sbom.json)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/sbom.json}"
echo "[generate-sbom] -> $OUT"
# Prefer cargo cyclonedx if available, else cargo metadata + npm ls fallback
if command -v cargo-cyclonedx >/dev/null 2>&1; then
  cargo cyclonedx --format json --override-filename "$OUT" 2>/dev/null || true
fi
# Build minimal CycloneDX SBOM from cargo metadata + frontend
RUST_PKGS="$(cargo metadata --format-version=1 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
    pkgs=d.get('packages',[])
    print(json.dumps([{'name':p['name'],'version':p['version'],'license':p.get('license','')} for p in pkgs[:200]]))
except Exception:
    print('[]')
" 2>/dev/null || echo '[]')"

# The web control plane lives at apps/control-plane; its dependency set is part
# of the delivered bill of materials.
FRONTEND_DEPS="[]"
PKG_JSON="$ROOT/apps/control-plane/package.json"
if [ -f "$PKG_JSON" ]; then
  FRONTEND_DEPS="$(python3 - "$PKG_JSON" <<'PY'
import json, pathlib, sys
try:
    p = json.loads(pathlib.Path(sys.argv[1]).read_text())
    deps = {**p.get("dependencies", {}), **p.get("devDependencies", {})}
    print(json.dumps([{"name": k, "version": v} for k, v in deps.items()]))
except Exception:
    print("[]")
PY
)"
fi

TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
VERSION="$(cat "$ROOT/VERSION" 2>/dev/null | tr -d ' \n' || echo "0.1.0")"

python3 - "$OUT" "$RUST_PKGS" "$FRONTEND_DEPS" "$TIMESTAMP" "$VERSION" << 'PY'
import hashlib
import json
import pathlib
import sys

out, rust_pkgs_s, frontend_s, ts, version = sys.argv[1:6]
rust_pkgs = json.loads(rust_pkgs_s)
frontend = json.loads(frontend_s)
components = []
for p in rust_pkgs:
    components.append({
        "type": "library",
        "name": p["name"],
        "version": p["version"],
        "purl": f"pkg:cargo/{p['name']}@{p['version']}",
        "licenses": p.get("license", ""),
    })
for p in frontend:
    components.append({
        "type": "library",
        "name": p["name"],
        "version": p["version"],
        "purl": f"pkg:npm/{p['name']}@{p['version']}",
    })
bom = {
    "bomFormat": "CycloneDX",
    "specVersion": "1.5",
    "version": 1,
    "metadata": {
        "timestamp": ts,
        "component": {"name": "sniper-suite", "version": version, "type": "application"},
    },
    "components": sorted(components, key=lambda x: x["name"]),
}
if not bom["components"]:
    # A BOM with no components is not evidence of anything: fail loudly rather
    # than shipping an empty inventory that reads as "no dependencies".
    raise SystemExit("[generate-sbom] refuse to write an SBOM with zero components")
pathlib.Path(out).write_text(json.dumps(bom, indent=2, sort_keys=True))
print(f"[generate-sbom] wrote {len(components)} components")
data = pathlib.Path(out).read_bytes()
sha = hashlib.sha256(data).hexdigest()
print(f"{sha}  {out}  size={len(data)}  ts={ts}")
PY
echo "[generate-sbom] OK"
