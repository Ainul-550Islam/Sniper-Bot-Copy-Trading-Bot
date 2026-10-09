#!/usr/bin/env bash
# ============================================================================
# scripts/export-openapi.sh — regenerate the committed API contract (P1).
#
#   ./scripts/export-openapi.sh            # regenerate both files
#   ./scripts/export-openapi.sh --check    # fail if either is stale (CI)
#
# `openapi/openapi.json` is AUTHORITATIVE and is produced by the Rust
# test `crates/server/tests/openapi_artifact.rs`, which calls the exact
# same `saas::openapi::document()` the server serves at
# `/api/saas/openapi.json`. There is therefore no second implementation
# that could drift.
#
# `openapi/openapi.yaml` is a straight transcode of that JSON, for the
# many tools that only read YAML. It is generated, never hand-edited.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

JSON="openapi/openapi.json"
YAML="openapi/openapi.yaml"
CHECK=0
[[ "${1:-}" == "--check" ]] && CHECK=1

fail() { echo "FAIL: $*" >&2; exit 1; }

# --- 1. JSON, from the code -------------------------------------------------
if [[ "$CHECK" -eq 1 ]]; then
    echo "==> verifying $JSON against the code"
    cargo test -p sniper-suite --test openapi_artifact --quiet \
        || fail "$JSON is stale — run ./scripts/export-openapi.sh and commit the result"
else
    echo "==> generating $JSON from saas::openapi::document()"
    UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact --quiet \
        || fail "could not generate $JSON"
fi
[[ -f "$JSON" ]] || fail "$JSON was not produced"

# --- 2. YAML, transcoded ----------------------------------------------------
# PyYAML rather than a Rust YAML crate on purpose: `serde_yaml` is
# unmaintained and would be flagged by `cargo audit`, and adding an
# advisory-flagged dependency to a trading workspace to pretty-print a
# document is a bad trade. python3 + PyYAML is present on every CI
# runner this repository targets.
command -v python3 >/dev/null 2>&1 || fail "python3 is required to transcode the YAML artifact"
python3 - "$JSON" "$YAML" "$CHECK" <<'PY'
import json, sys, pathlib
try:
    import yaml
except ImportError:
    sys.exit("FAIL: PyYAML is required (pip install pyyaml)")

src, dst, check = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3] == "1"
doc = json.loads(src.read_text())

header = (
    "# GENERATED FILE — DO NOT EDIT.\n"
    "# Source of truth: openapi/openapi.json, produced from\n"
    "# crates/server/src/saas/openapi.rs::document() by\n"
    "# ./scripts/export-openapi.sh. Edit the Rust, not this file.\n"
)
# sort_keys=True so the output is byte-stable across runs and a diff
# always means a real contract change.
body = yaml.safe_dump(doc, sort_keys=True, default_flow_style=False,
                      allow_unicode=True, width=100)
want = header + body

if check:
    if not dst.exists():
        sys.exit(f"FAIL: {dst} is missing — run ./scripts/export-openapi.sh")
    if dst.read_text() != want:
        sys.exit(f"FAIL: {dst} is stale — run ./scripts/export-openapi.sh and commit it")
    print(f"  ok   {dst} is current")
else:
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(want)
    print(f"  wrote {dst}")
PY

# --- 3. summary -------------------------------------------------------------
paths="$(python3 -c "import json,sys;print(len(json.load(open('$JSON'))['paths']))")"
version="$(python3 -c "import json;print(json.load(open('$JSON'))['info']['version'])")"
echo
echo "contract version : $version"
echo "paths documented : $paths"
echo "files            : $JSON, $YAML"
echo
echo "Version policy: docs/API-VERSIONING.md"
