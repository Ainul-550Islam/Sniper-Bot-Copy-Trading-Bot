#!/usr/bin/env bash
# Contract gate for the customer-facing routes that are easy to orphan when
# a page or handler is renamed. It checks both sides of each declared pair;
# OpenAPI remains the authoritative full route contract.
#
# Part 04 (L6) addition: a bidirectional OpenAPI <-> frontend gate implemented
# in embedded python3 (cargo-free, same style as scripts/check-openapi-coverage.sh):
#   (a) every /api/... literal the frontend actually calls must match an
#       OpenAPI path or be a prefix of one, and
#   (b) every OpenAPI path under /api/saas/** and /api/tenant/** must have at
#       least one frontend reference, unless it is in UI_EXEMPT below with a
#       reason.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER="$ROOT/crates/server/src"
UI="$ROOT/apps/control-plane/src"

pairs=(
  "/api/saas/activity|activity"
  "/api/saas/alerts|alerts"
  "/api/saas/portfolio|portfolio"
  "/api/saas/support/tickets|support"
  "/api/saas/pricing|pricing"
  "/api/saas/security/status|security"
  "/api/saas/team/invites|team"
  "/api/saas/team/invites/accept|accept-invite"
  "/api/saas/risk-dashboard|risk"
  "/api/tenant/analytics|analytics"
  "/api/tenant/integrations|integrations"
  "/api/tenant/onboarding|onboarding"
  "/api/tenant/sniper/config|sniper/config"
  "/api/tenant/copy/config|copy/config"
  "/api/tenant/polymarket/config|polymarket/config"
  "/api/tenant/dca-schedules|dca"
  "/api/tenant/limit-orders|limit-orders"
  "/api/saas/wallet-pools|wallet-pools"
  "/api/saas/data-lifecycle|data-lifecycle"
  "/api/saas/custody/profiles|custody"
  "/api/modules|modules"
  "/api/kill|kill-switch"
  "/api/mode|mode"
  "/api/keys|keys"
  "/api/audit|audit"
  "/api/journal|journal"
  "/api/reconciliation/findings|reconciliation"
  "/api/recovery/failed|recovery"
  "/api/config|config"
)

failed=0
for pair in "${pairs[@]}"; do
  route="${pair%%|*}"
  label="${pair##*|}"
  if ! grep -RFlq -- "$route" "$SERVER"; then
    echo "missing server route: $route ($label)" >&2
    failed=1
  fi
  if ! grep -RFlq -- "$route" "$UI"; then
    echo "missing frontend client reference: $route ($label)" >&2
    failed=1
  fi
done

if (( failed )); then
  exit 1
fi
echo "check-routes-vs-ui: declared server/UI route pairs are present"

# ---------------------------------------------------------------------------
# Part 04 L6: OpenAPI <-> frontend coverage gate.
# ---------------------------------------------------------------------------
SPEC="$ROOT/openapi/openapi.json"

ROOT="$ROOT" SPEC="$SPEC" UI="$UI" python3 - <<'PY'
import json
import os
import re
import sys
from pathlib import Path

root = Path(os.environ["ROOT"])
spec_path = Path(os.environ["SPEC"])
SRC = Path(os.environ["UI"])

if not spec_path.exists():
    print(f"check-routes-vs-ui: FAIL — {spec_path} missing", file=sys.stderr)
    sys.exit(1)

# ---------------------------------------------------------------------------
# UI_EXEMPT: the ONLY OpenAPI paths allowed to have no frontend reference.
# Each entry is "path|reason". Nothing else may be exempt.
# ---------------------------------------------------------------------------
UI_EXEMPT_RAW = (
    "/api/saas/billing/webhooks/{provider}|provider webhook callbacks are invoked by the billing provider, not by the UI",
    "/api/saas/billing/payment-webhooks/{provider}|provider webhook callbacks are invoked by the billing provider, not by the UI",
    "/api/saas/sso/callback|SSO callback is handled by the backend session flow, not by the UI",
    "/api/ops/migration-health|operator-only background endpoint, never called by the UI",
    "/api/saas/lifecycle/jobs/{id}/advance|operator-only job advancement, never called by the UI",
)
UI_EXEMPT = {}
for entry in UI_EXEMPT_RAW:
    path, _, reason = entry.partition("|")
    UI_EXEMPT[path.rstrip("/")] = reason

SCOPES = (
    "/api/saas/",
    "/api/tenant/",
    "/api/health",
    "/api/mode",
    "/api/modules",
    "/api/kill",
    "/api/resume",
    "/api/risk/kill-switch",
    "/api/keys",
    "/api/audit",
    "/api/journal",
    "/api/db",
    "/api/config",
    "/api/reconciliation/findings",
    "/api/recovery/failed",
    "/api/executions",
    "/api/trades",
    "/api/wallets",
    "/api/accounting/events",
    "/api/events",
    "/api/orders",
)

def in_scope(path: str) -> bool:
    return any(path.startswith(s) for s in SCOPES)

# ---------------------------------------------------------------------------
# 1. Strip comments (string-aware) and extract every /api/... literal the
#    frontend calls, normalized the same way the api-contract vitest gate does:
#    ${…} -> {x}, query string dropped, trailing glued {x} dropped.
# ---------------------------------------------------------------------------
def strip_comments(src: str) -> str:
    out = []
    i = 0
    n = len(src)
    quote = None
    while i < n:
        ch = src[i]
        if quote:
            if ch == "\\":
                out.append(ch)
                out.append(src[i + 1] if i + 1 < n else "")
                i += 2
                continue
            if ch == quote:
                quote = None
            out.append(ch)
            i += 1
            continue
        if ch in ('"', "'", "`"):
            quote = ch
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
            continue
        if ch == "/" and i + 1 < n and src[i + 1] == "*":
            out.append("  ")
            i += 2
            while i < n and not (src[i] == "*" and i + 1 < n and src[i + 1] == "/"):
                out.append(src[i] if src[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append("  ")
                i += 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)

LITERAL_RE = re.compile(r'([\'`"])((?:\\.|(?!\1).)*)\1')
PREFIX_CONSTANTS = {"/api/tenant"}

SKIP_PARTS = {"__tests__", "node_modules", ".next"}

frontend = set()
for ts_file in SRC.rglob("*"):
    if any(part in SKIP_PARTS for part in ts_file.parts):
        continue
    if ts_file.is_dir():
        continue
    if ts_file.suffix not in (".ts", ".tsx"):
        continue
    source = strip_comments(ts_file.read_text(encoding="utf8", errors="replace"))
    for match in LITERAL_RE.finditer(source):
        body = match.group(2) or ""
        if not body.startswith("/api/"):
            continue
        path = re.sub(r"\$\{[^}]*\}", "{x}", body).split("?")[0]
        if path.endswith("{x}") and not path.endswith("/{x}"):
            path = path[: -len("{x}")]
        path = path.rstrip("/")
        if not path or path in PREFIX_CONSTANTS:
            continue
        frontend.add(path)

# ---------------------------------------------------------------------------
# 2. Segment matching: {x} on either side matches any single segment.
#    A frontend path references a spec path when they match segment-wise,
#    or when the frontend path is a prefix of the spec path (a
#    `${suffix}`-style concat prefix counts as a reference).
# ---------------------------------------------------------------------------
def segments_match(candidate: str, template: str) -> bool:
    a = candidate.split("/")
    b = template.split("/")
    if len(a) != len(b):
        return False
    return all(
        x == y or x.startswith("{") or y.startswith("{")
        for x, y in zip(a, b)
    )

def references(candidate: str, template: str) -> bool:
    if segments_match(candidate, template):
        return True
    a = candidate.split("/")
    b = template.split("/")
    if len(a) > len(b):
        return False
    return all(
        x == y or x.startswith("{") or y.startswith("{")
        for x, y in zip(a, b)
    )

# ---------------------------------------------------------------------------
# 3. Bidirectional comparison.
# ---------------------------------------------------------------------------
spec = json.loads(spec_path.read_text(encoding="utf8"))
spec_paths = {p.rstrip("/") for p in spec.get("paths", {})}
spec_scoped = {p for p in spec_paths if in_scope(p)}

# (a) every frontend path must match an OpenAPI path or prefix thereof
ghost = sorted(
    c
    for c in frontend
    if not any(references(c, p) for p in spec_paths)
)

# (b) every scoped spec path needs a frontend reference unless exempt
missing = []
for p in sorted(spec_scoped):
    if p in UI_EXEMPT:
        continue
    if not any(references(c, p) for c in frontend):
        missing.append((p, UI_EXEMPT.get(p)))

print("check-routes-vs-ui: OpenAPI <-> frontend gate")
print(f"  frontend /api/ literals scanned : {len(frontend)}")
print(f"  scoped OpenAPI paths            : {len(spec_scoped)}")

failed = False
if ghost:
    failed = True
    print("\nFAIL: frontend API paths with no OpenAPI coverage:", file=sys.stderr)
    for p in ghost:
        print(f"  {p}", file=sys.stderr)

if missing:
    failed = True
    print("\nFAIL: scoped OpenAPI paths with no frontend reference (not exempt):", file=sys.stderr)
    for p, _reason in missing:
        print(f"  {p}", file=sys.stderr)

if not failed:
    print("check-routes-vs-ui: OK — every frontend API path is documented and every")
    print("  scoped OpenAPI path is referenced by the frontend (except the UI_EXEMPT list).")
    sys.exit(0)

print("\nFix the frontend call (or the server route) — only the five UI_EXEMPT paths", file=sys.stderr)
print("may be exempt, each with a reason.", file=sys.stderr)
sys.exit(1)
PY
