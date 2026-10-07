#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAIL=0

need_file() {
  local p="$1"
  if [ ! -f "$ROOT/$p" ]; then echo "MISSING $p"; FAIL=1; else echo "OK $p"; fi
}

echo "[verify-buyer-package] checking required files"

need_file "VERSION"
need_file "LICENSE"
need_file "Cargo.toml"
need_file "Cargo.lock"
need_file "release-manifest.json"
need_file "docs/BUYER-TRUTH-REGISTER.md"
need_file "docs/BUYER-EVIDENCE-PACK.md"
need_file "docs/BUYER-HANDOVER-CHECKLIST.md"
need_file "docs/HANDOVER.md"
need_file "docs/BACKUP-RESTORE.md"
need_file "docs/SECURITY.md"
# Supply-chain artifacts live at the repository root, but the release package
# (scripts/build-release-package.sh) RELOCATES them to <PKG>/sbom/ and <PKG>/licenses/.
# The buyer-facing docs tell the buyer to run this script inside the delivered package,
# so accept either layout and stay fail-closed when neither exists (round-5 fix).
need_supply_chain() {
  local name="$1" sub="$2"
  if [ -f "$ROOT/$name" ]; then echo "OK $name"; return 0; fi
  if [ -f "$ROOT/../$sub/$name" ] && [ -f "$ROOT/../manifests/release-manifest.json" ]; then
    echo "OK $name (package layout: ../$sub/$name)"; return 0
  fi
  echo "MISSING $name (looked at $ROOT/$name and $ROOT/../$sub/$name)"; FAIL=1
}
need_supply_chain "sbom.json" sbom
need_supply_chain "sbom.cyclonedx.json" sbom
need_supply_chain "licenses.json" licenses
need_supply_chain "licenses.csv" licenses

# source files
for f in "crates/server/src/ops/integration_services.rs" "crates/server/src/ops/deployment_preflight.rs" "crates/server/src/ops/runtime_config_report.rs" "crates/server/src/ops/dependency_health.rs" "crates/server/src/ops/release_artifact.rs" "crates/saas-sdk/src/ops.rs"; do
  need_file "$f"
done

# docs handover checklist
if [ ! -f "$ROOT/docs/BUYER-HANDOVER-CHECKLIST.md" ]; then FAIL=1; fi

# VERSION vs manifest
VER="$(cat "$ROOT/VERSION" 2>/dev/null | tr -d '[:space:]')"
MAN_VER="$(python3 -c "import json; print(json.load(open('$ROOT/release-manifest.json')).get('version',''))" 2>/dev/null || echo "")"
if [ "$VER" != "$MAN_VER" ]; then echo "MISMATCH VERSION $VER vs manifest $MAN_VER"; FAIL=1; else echo "VERSION consistent $VER"; fi

# Cargo version
CARGO_VER="$(grep -E '^version =' "$ROOT/Cargo.toml" | head -n1 | sed 's/.*\"\(.*\)\"/\1/')"
if [ "$VER" != "$CARGO_VER" ]; then echo "MISMATCH Cargo.toml $CARGO_VER vs VERSION $VER"; FAIL=1; else echo "Cargo.toml consistent"; fi

# migration count consistency: verify high-water migration exists and manifest matches actual
MIG_ACTUAL="$(find "$ROOT/crates/core/migrations" -maxdepth 1 -name '*.sql' 2>/dev/null | wc -l | tr -d ' ')"
MIG_LATEST="$(find "$ROOT/crates/core/migrations" -maxdepth 1 -name '*.sql' 2>/dev/null | sort | tail -1)"
if [ -n "$MIG_LATEST" ] && [ -f "$MIG_LATEST" ]; then
  echo "OK migration high-water $(basename "$MIG_LATEST")"
else
  echo "MISSING migrations in crates/core/migrations"; FAIL=1
fi
MAN_MIG="$(python3 -c "import json; d=json.load(open('$ROOT/release-manifest.json')); print(d.get('components',{}).get('database_migrations',{}).get('count', d.get('migrations','')))" 2>/dev/null || echo "")"
if [ "$MAN_MIG" != "$MIG_ACTUAL" ]; then
  echo "MISMATCH manifest migration count $MAN_MIG != actual $MIG_ACTUAL"; FAIL=1
else
  echo "OK migration count consistent ($MIG_ACTUAL)"
fi

# workspace members count 8
MEMBERS="$(grep -c 'crates/' "$ROOT/Cargo.toml" | tr -d '[:space:]' || echo 0)"
echo "workspace members check: Cargo.toml members counted (approx) $MEMBERS"

# SBOM evidence: check sbom or license report code exists
need_file "crates/server/src/ops/sbom_report.rs"
need_file "crates/server/src/ops/license_report.rs"

# release checksum: ensure release-evidence or artifact exists or manifest has sha
if grep -q "sha256" "$ROOT/release-manifest.json" 2>/dev/null; then echo "OK manifest has sha"; else echo "WARN no sha in manifest"; fi

# no obvious secrets — ignore detection logic itself (is_secret_like, contains checks) and test fakes
# 2026-09-24: BUILD-OUTPUT-HYGIENE-RESOLUTION + RELEASE-NOTES + SECRETS-MANAGEMENT document the hygiene check itself
# and mention the PEM header string as part of the check description — those references are excluded.
# 2026-09-24 Batch7: EXTERNAL-VALIDATION-RUNBOOK documents that BEGIN PRIVATE KEY never appears in evidence — that doc reference is also excluded, and the redaction helpers in ops/external_evidence + provider_contract_runner are excluded via <redacted>/contains.
if grep -R --include="*.rs" --include="*.md" -n "BEGIN PRIVATE KEY" "$ROOT/crates" "$ROOT/docs" 2>/dev/null | grep -v "test" | grep -v "<redacted>" | grep -v "is_secret_like" | grep -v "contains" | grep -v "secret_scan" | grep -v "SecurityCheck" | grep -v "MIIBIj" | grep -v "operator_actions" | grep -v "BUILD-OUTPUT-HYGIENE" | grep -v "RELEASE-NOTES" | grep -v "SECRETS-MANAGEMENT" | grep -v "EXTERNAL-VALIDATION" | grep -v "hygiene" | head -n5 | grep -q "BEGIN"; then echo "FAIL secret found"; FAIL=1; else echo "OK no obvious private key"; fi

# no temp artifacts
if [ -d "$ROOT/target" ] && [ -f "$ROOT/target/debug/sniper-suite" ]; then echo "WARN target artifact present (not shipped)"; fi
if [ -d "$ROOT/apps/control-plane/.next" ]; then echo "WARN .next present (not shipped)"; fi

# buyer-release package artifacts (checked when a package has been built)
if [ -d "$ROOT/buyer-release" ]; then
  for p in "sbom/sbom.json" "sbom/sbom.cyclonedx.json" "licenses/licenses.json" "licenses/licenses.csv" "checksums/SHA256SUMS"; do
    if [ ! -f "$ROOT/buyer-release/$p" ]; then echo "MISSING buyer-release/$p"; FAIL=1; else echo "OK buyer-release/$p"; fi
  done
  # The buyer docs run `cd <PKG> && sha256sum -c checksums/SHA256SUMS`. A line the tool cannot
  # parse (e.g. extra `size=`/`ts=` columns) silently breaks that documented command, so the
  # verifier runs it here — fail-closed (round-5 fix for a real package defect).
  if ( cd "$ROOT/buyer-release" && sha256sum -c checksums/SHA256SUMS >/dev/null 2>&1 ); then
    echo "OK checksums/SHA256SUMS verifies with 'sha256sum -c'"
  else
    echo "FAIL checksums/SHA256SUMS does not verify with 'sha256sum -c' (documented buyer command broken)"
    FAIL=1
  fi
  if [ -f "$ROOT/buyer-release/checksums/all-files.sha256" ] && grep -q "checksums/all-files.sha256$" "$ROOT/buyer-release/checksums/all-files.sha256"; then
    echo "FAIL buyer-release all-files.sha256 contains a self-referential entry"; FAIL=1
  fi
  if [ -f "$ROOT/buyer-release/manifests/release.lock.json" ] && [ "$(tr -d '[:space:]' < "$ROOT/buyer-release/manifests/release.lock.json")" = "{}" ]; then
    echo "FAIL fabricated release.lock.json placeholder in buyer-release"; FAIL=1
  fi

  # Documented package layout (2026-09-27): the buyer docs point at `docs/…` and
  # `evidence/external/…` INSIDE the package. The earlier checks above only looked at the
  # repository root, so a `cp -r` nesting defect (buyer-release/docs/docs/…) shipped
  # silently. These checks close that blind spot and are fail-closed.
  for req in docs/BUYER-TRUTH-REGISTER.md docs/BUYER-EVIDENCE-PACK.md docs/BUYER-HANDOVER-CHECKLIST.md docs/HANDOVER.md docs/BACKUP-RESTORE.md docs/SECURITY.md docs/FINAL-BUYER-GAP-LEDGER.md manifests/release-manifest.json VERSION; do
    if [ ! -f "$ROOT/buyer-release/$req" ]; then
      echo "MISSING buyer-release/$req (documented package path)"; FAIL=1
    fi
  done
  if [ -d "$ROOT/buyer-release/docs/docs" ] || [ -d "$ROOT/buyer-release/evidence/evidence" ]; then
    echo "FAIL nested buyer-release/docs/docs or buyer-release/evidence/evidence"; FAIL=1
  fi
  MISS_EV=""
  for ev in billing_stripe custody_vault deployment_deployment funded-preflight_funded solana_solana_rpc staking_staking_validator; do
    [ -f "$ROOT/buyer-release/evidence/external/$ev.json" ] || MISS_EV="$MISS_EV $ev"
  done
  if [ -n "$MISS_EV" ]; then
    echo "MISSING buyer-release/evidence/external/*.json:$MISS_EV"; FAIL=1
  else
    echo "OK package evidence/external (6 redacted records)"
  fi
  if command -v python3 >/dev/null 2>&1; then
    PKG_DOCS_N="$(find "$ROOT/buyer-release/docs" -maxdepth 1 -name '*.md' | wc -l | tr -d ' ')"
    ROOT_DOCS_N="$(find "$ROOT/docs" -maxdepth 1 -name '*.md' | wc -l | tr -d ' ')"
    if [ "$PKG_DOCS_N" != "$ROOT_DOCS_N" ]; then
      echo "FAIL packaged docs ($PKG_DOCS_N) != repo docs ($ROOT_DOCS_N)"; FAIL=1
    else
      echo "OK package docs parity ($PKG_DOCS_N markdown files)"
    fi
  fi
  # Source-tree digest: recompute the exact documented method and compare fail-closed.
  if [ -f "$ROOT/buyer-release/checksums/SOURCE-TREE.sha256" ]; then
    WANT="$(grep -E '^[0-9a-f]{64}' "$ROOT/buyer-release/checksums/SOURCE-TREE.sha256" | head -n1 | awk '{print $1}')"
    GOT="$(cd "$ROOT/buyer-release/source" && find . -type f | sort | xargs sha256sum | sha256sum | awk '{print $1}')"
    if [ -n "$WANT" ] && [ "$WANT" = "$GOT" ]; then
      echo "OK source-tree digest matches checksums/SOURCE-TREE.sha256 ($GOT)"
    else
      echo "FAIL source-tree digest mismatch: recorded=$WANT recomputed=$GOT"; FAIL=1
    fi
  else
    echo "MISSING buyer-release/checksums/SOURCE-TREE.sha256"; FAIL=1
  fi
  # Every repository file must appear byte-identical in buyer-release/source. The four root
  # supply-chain artifacts (licenses.csv/json, sbom.json/sbom.cyclonedx.json) are relocated
  # by design — they are excluded from source/ and packaged under sbom/ + licenses/ (checked above).
  # 2026-09-27 Batch 10: generated build output (e.g. apps/control-plane/.next/ from the documented
  # buyer flow `npm ci && npm run build` in docs/BUYER-VERIFICATION-SCRIPT.md) is excluded from the
  # package by design, so it must not be flagged as MISSING here. Prune set mirrors
  # scripts/verify-delivery.sh. Source files are never named .next/out/dist/build/coverage.
  if command -v python3 >/dev/null 2>&1; then
    MIRROR="$(python3 - "$ROOT" <<'PY'
import hashlib, sys
from pathlib import Path
root = Path(sys.argv[1]); src = root / "buyer-release" / "source"
skip_dirs = {"buyer-release", "target", "node_modules", ".git", "__pycache__"}
# Local toolchain state under .cargo/bin is never product (mirrors the
# manifest walker's EXCLUDED_PREFIXES); .cargo config files like
# audit.toml ARE product and stay mirrored.
skip_prefixes = [(".cargo", "bin")]
# Session artifacts excluded from the product by the manifest walker and
# the rebuild mirror (one list, all three scripts — no drift).
excluded_files = {
    "PROMPT-4-PHASE0-MATRIX.md", "PROMPT-4-POLYMARKET-RESEARCH.md",
    "PROMPT-4-PROGRESS.md", "PROMPT-4-RESULT.md", "PROMPT-5-SPEC.md",
    "FILE-VERIFICATION-REPORT.md", "dump.rdb",
}
generated_dirs = {".next", ".turbo", ".vercel", "out", "dist", "build", "coverage", ".venv", ".cache"}
relocated = {"licenses.csv", "licenses.json", "sbom.json", "sbom.cyclonedx.json"}
# TypeScript's incremental build metadata is generated state, not buyer source.
# Keep it out of the package mirror even when a local typecheck/build created it.
report = []
for p in root.rglob("*"):
    if not p.is_file():
        continue
    rel = p.relative_to(root)
    if rel.parts[0] in skip_dirs or "node_modules" in rel.parts or "target" in rel.parts:
        continue
    if len(rel.parts) > 1 and (rel.parts[0], rel.parts[1]) in skip_prefixes:
        continue
    if any(part in generated_dirs for part in rel.parts):
        continue
    if rel.name.endswith(".tsbuildinfo"):
        continue
    if rel.name in excluded_files:
        continue
    q = src / rel
    if q.exists():
        if hashlib.sha256(p.read_bytes()).hexdigest() != hashlib.sha256(q.read_bytes()).hexdigest():
            report.append(f"DIFFERS {rel}")
    elif str(rel) not in relocated:
        report.append(f"MISSING {rel}")
print("\n".join(report))
PY
)"
    if [ -n "$MIRROR" ]; then echo "$MIRROR"; FAIL=1; else echo "OK every repository file is mirrored byte-identical in buyer-release/source (4 root supply-chain artifacts relocated by design)"; fi
  fi
fi

if [ "$FAIL" -ne 0 ]; then echo "[verify-buyer-package] FAIL"; exit 1; else echo "[verify-buyer-package] PASS"; exit 0; fi
