#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/licenses.json}"
CSV="${2:-$ROOT/licenses.csv}"
echo "[generate-license-report] -> $OUT , $CSV"
# Try cargo deny / cargo license; fallback to cargo metadata license fields
if command -v cargo-license >/dev/null 2>&1; then
  cargo license --json 2>/dev/null | python3 -m json.tool > "$OUT" 2>/dev/null || true
fi
# Build license report from cargo metadata if not already populated
if [ ! -s "$OUT" ] || [ "$(wc -c < "$OUT" | tr -d ' ')" -lt 10 ]; then
  cargo metadata --format-version=1 2>/dev/null | python3 -c "
import json, sys, pathlib
d=json.load(sys.stdin)
pkgs=d.get('packages',[])
rows=[]
for p in pkgs:
    rows.append({'name':p['name'],'version':p['version'],'license':p.get('license') or 'UNKNOWN','repository':p.get('repository') or ''})
rows=sorted(rows, key=lambda x: x['name'].lower())
pathlib.Path('$OUT').write_text(json.dumps(rows, indent=2, sort_keys=True))
" 2>/dev/null || echo '[]' > "$OUT"
fi

# CSV projection
python3 - "$OUT" "$CSV" << 'PY'
import json, pathlib, csv, sys, hashlib, datetime
out, csv_path = sys.argv[1:3]
rows = json.loads(pathlib.Path(out).read_text())
with open(csv_path, 'w', newline='') as f:
    w = csv.DictWriter(f, fieldnames=['name','version','license','repository'])
    w.writeheader()
    for r in rows:
        w.writerow({k: r.get(k,'') for k in ['name','version','license','repository']})
print(f"[generate-license-report] wrote {len(rows)} entries")
for path in [out, csv_path]:
    data = pathlib.Path(path).read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    sz = len(data)
    ts = datetime.datetime.utcnow().strftime('%Y-%m-%dT%H:%M:%SZ')
    print(f"{sha}  {path}  size={sz}  ts={ts}")
PY
# Also handle frontend licenses if npm present
if [ -f "$ROOT/frontend/package-lock.json" ] && command -v npx >/dev/null 2>&1; then
  echo "[generate-license-report] frontend: npx license-checker --summary (best-effort)"
  (cd "$ROOT/frontend" && npx --yes license-checker --summary 2>/dev/null | head -n 100 || true)
fi
echo "[generate-license-report] OK"
cat "$OUT" | head -n 80
