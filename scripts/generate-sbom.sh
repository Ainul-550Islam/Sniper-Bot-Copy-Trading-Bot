#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/sbom.json}"
CYCLONE="${2:-$ROOT/sbom.cyclonedx.json}"
echo "[generate-sbom] -> $OUT , $CYCLONE"
# Prefer cargo cyclonedx if available, else cargo metadata + npm ls fallback
if command -v cargo-cyclonedx >/dev/null 2>&1; then
  cargo cyclonedx --format json --override-filename "$CYCLONE" 2>/dev/null || true
fi
# Build minimal CycloneDX-like SBOM from cargo metadata + frontend
CARGO_META="$(cargo metadata --format-version=1 --no-deps 2>/dev/null | head -c 200000 || echo '{}')"
# Extract rust packages
RUST_PKGS="$(cargo metadata --format-version=1 2>/dev/null | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
    pkgs=d.get('packages',[])
    print(json.dumps([{'name':p['name'],'version':p['version'],'license':p.get('license','')} for p in pkgs[:200]]))
except Exception as e:
    print('[]')
" 2>/dev/null || echo '[]')"

FRONTEND_DEPS="[]"
if [ -f "$ROOT/frontend/package.json" ]; then
  FRONTEND_DEPS="$(python3 -c "
import json, pathlib
try:
    p=json.loads(pathlib.Path('$ROOT/frontend/package.json').read_text())
    deps={**p.get('dependencies',{}), **p.get('devDependencies',{})}
    print(json.dumps([{'name':k,'version':v} for k,v in deps.items()]))
except: print('[]')
" 2>/dev/null || echo '[]')"
fi

TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
VERSION="$(cat "$ROOT/VERSION" 2>/dev/null | tr -d ' \n' || echo "0.1.0")"

python3 - "$OUT" "$CYCLONE" "$RUST_PKGS" "$FRONTEND_DEPS" "$TIMESTAMP" "$VERSION" << 'PY'
import json, sys, hashlib, pathlib
out, cyclone, rust_pkgs_s, frontend_s, ts, version = sys.argv[1:7]
rust_pkgs = json.loads(rust_pkgs_s)
frontend = json.loads(frontend_s)
components = []
for p in rust_pkgs:
    components.append({"type":"library","name":p["name"],"version":p["version"],"purl":f"pkg:cargo/{p['name']}@{p['version']}","licenses":p.get("license","")})
for p in frontend:
    components.append({"type":"library","name":p["name"],"version":p["version"],"purl":f"pkg:npm/{p['name']}@{p['version']}"})
bom = {
    "bomFormat":"CycloneDX",
    "specVersion":"1.5",
    "version":1,
    "metadata":{"timestamp":ts,"component":{"name":"sniper-suite","version":version,"type":"application"}},
    "components": sorted(components, key=lambda x: x["name"])
}
pathlib.Path(out).write_text(json.dumps(bom, indent=2, sort_keys=True))
# also write cyclone separately (same content)
pathlib.Path(cyclone).write_text(json.dumps(bom, indent=2, sort_keys=True))
print(f"[generate-sbom] wrote {len(components)} components")
# also compute per-artifact sha256+size+timestamp
for path in [out, cyclone]:
    data = pathlib.Path(path).read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    sz = len(data)
    print(f"{sha}  {path}  size={sz}  ts={ts}")
PY
echo "[generate-sbom] OK"
