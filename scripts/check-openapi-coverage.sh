#!/usr/bin/env bash
# check-openapi-coverage.sh — OpenAPI coverage gate (GAP MAP v2, Part 5).
#
# Compares the routes actually registered in the axum router against the paths
# documented in openapi/openapi.json, SCOPED to the customer-facing API:
#   /api/saas/…   and   /api/tenant/…
# Operator surfaces (/api/ops, /api/admin, /api/webhooks provider callbacks,
# static /api/openapi.json) are intentionally OUT of scope — they are internal
# and are not part of the public control-plane contract.
#
# Fails (exit 1) when:
#   - a scoped route is registered but missing from the spec (undocumented), or
#   - a scoped spec path has no matching registered route (ghost path).
# Path parameters are normalized both ways (axum ":id" == OpenAPI "{id}").
#
# This is the cargo-free CI twin of the Rust
# `openapi_router_conformance` dynamic test; both must agree.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/openapi/openapi.json"

if [ ! -f "$SPEC" ]; then
  echo "check-openapi-coverage: FAIL — $SPEC missing (run scripts/regen-openapi-artifact.py)" >&2
  exit 1
fi

ROOT="$ROOT" SPEC="$SPEC" python3 - <<'PY'
import json
import os
import re
import sys
from pathlib import Path

root = Path(os.environ["ROOT"])
spec_path = Path(os.environ["SPEC"])
SRC = root / "crates/server/src"

SCOPES = ("/api/saas/", "/api/tenant/")

def in_scope(p: str) -> bool:
    return any(p.startswith(s) for s in SCOPES)

# ---------------------------------------------------------------------------
# 1. Registered routes: every `.route("/api/…"` in the server crate sources.
# ---------------------------------------------------------------------------
ROUTE_RE = re.compile(r'\.route\(\s*"(/api/[^"]+)"', re.S)

registered = set()
for rs in SRC.rglob("*.rs"):
    text = rs.read_text(encoding="utf-8", errors="replace")
    for m in ROUTE_RE.finditer(text):
        registered.add(m.group(1))

def normalize(path: str) -> str:
    # axum :param -> OpenAPI {param}; strip trailing slash
    return re.sub(r":([A-Za-z0-9_]+)", r"{\1}", path).rstrip("/")

registered_scoped = {normalize(p) for p in registered if in_scope(p)}

# ---------------------------------------------------------------------------
# 2. Documented paths from the spec artifact.
# ---------------------------------------------------------------------------
spec = json.loads(spec_path.read_text())
documented = set(spec.get("paths", {}).keys())
documented_scoped = {p.rstrip("/") for p in documented if in_scope(p)}

# ---------------------------------------------------------------------------
# 3. Bidirectional comparison.
# ---------------------------------------------------------------------------
missing_from_spec = sorted(registered_scoped - documented_scoped)
ghost_paths = sorted(documented_scoped - registered_scoped)

print(f"check-openapi-coverage: scope = /api/saas/** + /api/tenant/**")
print(f"  registered scoped routes : {len(registered_scoped)}")
print(f"  documented scoped paths  : {len(documented_scoped)}")

if not missing_from_spec and not ghost_paths:
    print("check-openapi-coverage: OK — every scoped route is documented and every documented scoped path is routed.")
    sys.exit(0)

if missing_from_spec:
    print("\nFAIL: scoped routes with NO OpenAPI documentation:", file=sys.stderr)
    for p in missing_from_spec:
        print(f"  {p}", file=sys.stderr)
if ghost_paths:
    print("\nFAIL: documented scoped paths with NO registered route:", file=sys.stderr)
    for p in ghost_paths:
        print(f"  {p}", file=sys.stderr)
print(
    "\nFix in crates/server/src/saas/openapi*.rs fragments, then regenerate with "
    "scripts/regen-openapi-artifact.py (never hand-edit openapi/openapi.json).",
    file=sys.stderr,
)
sys.exit(1)
PY
