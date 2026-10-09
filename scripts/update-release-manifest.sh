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
# P0-B TASK 6: narrative/history blocks are archived out of the manifest
# into docs/archive/manifest-history-2026-10.json and replaced with the
# structured test_counts / verification_status / evidence_pointers /
# run_logs shapes (idempotent).
#
# Everything else (version, notes) stays as-is; this script
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
# Local toolchain state under .cargo/bin is never product. docs/archive
# is internal history, not product (P0-B TASK 1: keep every measuring
# script agreeing that archive files never count).
EXCLUDED_PREFIXES = [".cargo/bin", "docs/archive"]
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
        # TypeScript emits this local incremental cache during typecheck/build;
        # it is not product source and must never change release measurements.
        if rel.name.endswith(".tsbuildinfo"):
            continue
        yield rel

files = list(product_files())
# `rust_files` follows the established manifest/test contract: .rs files
# under crates/ ONLY (programs/ has its own lockfile and is counted as a
# component, not as workspace rust source).
rust_files = [f for f in files if f.suffix == ".rs" and f.parts[0] == "crates"]
# docs_files follows the canonical P0-B definition shared with
# tests/release/manifest_current.sh and scripts/verify-delivery.sh:
# TOP-LEVEL docs/*.md only. Archive history and non-md artefacts never
# count, so all three measurers agree by construction.
docs_files = [
    f for f in files
    if f.parts[0] == "docs" and len(f.parts) == 2 and f.suffix == ".md"
]

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

# --- P0-B TASK 6: prose/history leaves the manifest --------------------
# Every narrative block (batch delivery reports, old run-result lists,
# legacy test_counts) is archived to docs/archive/manifest-history-2026-10.json
# and replaced by structured shapes. This step is idempotent: once the
# prose is gone it stays gone, and the archive file only ever grows.
ARCHIVE_PATH = root / "docs" / "archive" / "manifest-history-2026-10.json"
history = {}
if ARCHIVE_PATH.is_file():
    try:
        history = json.loads(ARCHIVE_PATH.read_text())
    except Exception:
        history = {}
history.setdefault(
    "about",
    "Narrative/history blocks removed from release-manifest.json by "
    "scripts/update-release-manifest.sh (P0-B TASK 6, 2026-10-08). The "
    "manifest itself now carries only structured, recomputable fields. "
    "Run-result prose lives only here; no run logs ship in this tree.",
)
sections = history.setdefault("sections", {})


def archive(key, value):
    if value not in (None, "", [], {}):
        sections[key] = value


comps = manifest.get("components", {})
for k in sorted(list(comps)):
    if (
        k.endswith("_delivery")
        or k.startswith("saas_modules")
        or k in ("transaction_readiness_batch6_25",
                 "external_validation_batch7_24",
                 "api_endpoints_documented")
    ):
        archive("components." + k, comps.pop(k))
if isinstance(manifest.get("test_counts"), dict) and "workspace_total" in manifest.get("test_counts", {}):
    archive("test_counts_legacy", manifest.pop("test_counts"))
if isinstance(manifest.get("verification_status"), dict) and "verified_final_pass" in manifest.get("verification_status", {}):
    archive("verification_status_legacy", manifest.pop("verification_status"))
ver = manifest.get("verification", {})
for k in sorted(list(ver)):
    if k != "manifest":
        archive("verification." + k, ver.pop(k))
if sections:
    ARCHIVE_PATH.parent.mkdir(parents=True, exist_ok=True)
    ARCHIVE_PATH.write_text(json.dumps(history, indent=2) + "\n")

# Structured replacements ------------------------------------------------
manifest["license"] = "LicenseRef-Proprietary"
manifest["test_counts"] = {
    "method": "grep-count of '#[test]' attribute lines under crates/ (identical to test_count)",
    "value": test_count,
    "note": "Static inventory of test functions: a count, not a pass/fail "
            "result. No run logs ship in this package; re-run per docs/TESTING.md.",
}


def live_status(rel_path):
    p = root / rel_path
    if not p.is_file():
        return "NO_EVIDENCE"
    try:
        data = json.loads(p.read_text())
    except Exception:
        return "UNREADABLE"
    st = str(data.get("status", "")).upper()
    if st:
        return st
    result = data.get("result")
    if isinstance(result, dict):
        st = str(result.get("status", "")).upper()
        if st:
            return st
    return "UNKNOWN"


CLAIMS = [
    ("1", "Small funded Solana trade lands",
     "crates/solana-kit sign/send/confirm path; crates/solana-kit/tests/latency_bench.rs (simulate leg)",
     "evidence/live/solana_funded_preflight.json"),
    ("2", "pump.fun buy + sell round-trip",
     "crates/module-sniper entry/exit logic; tests/mock_pumpportal.rs",
     "evidence/live/pumpfun_buy_sell_roundtrip.json"),
    ("3", "PumpSwap buy + sell round-trip",
     "crates/module-sniper swap routing",
     "evidence/live/pumpswap_buy_sell_roundtrip.json"),
    ("4", "Polymarket place + cancel + fill",
     "crates/module-polymarket (builder.rs/clob.rs order + cancel paths) + mock test suite",
     "evidence/live/polymarket_order_roundtrip.json"),
    ("5", "Stripe checkout + webhook verify (test mode)",
     "live_billing_contract harness; docs/WEBHOOK-COMPATIBILITY-MATRIX.md fixture tests",
     "evidence/live/stripe_checkout_roundtrip.json"),
    ("6", "AWS KMS signing round-trip",
     "live_custody_contract harness; signer registry code",
     "evidence/live/kms_sign_transit.json"),
    ("7", "Vault Transit signing round-trip",
     "live_custody_contract harness; signer registry code",
     "evidence/live/vault_transit.json"),
    ("8", "Deployment smoke against a real deployment",
     "scripts/run-external-validation.sh (deployment_smoke op); docs/DEPLOYMENT.md",
     "evidence/live/deployment_smoke.json"),
    ("9", "Staking program e2e (devnet/validator)",
     "programs/staking-suite/tests/validator_e2e.rs (gated on STAKING_E2E=1)",
     "evidence/live/staking_devnet_e2e.json"),
    ("10", "Latency report (p50/p95 detect->submit->landed)",
     "crates/solana-kit/tests/latency_bench.rs (read-only + simulate legs)",
     "evidence/live/latency_report.json"),
    ("11", "One green CI run at the release commit",
     ".github/workflows/ci.yml exists",
     "evidence/live/ci_run.json"),
]
manifest["verification_status"] = {
    "policy": "PUBLIC-SAFE: a claim may appear in public/buyer wording only "
              "when its live_evidence_status is PASSED. Until then only the "
              "harness wording from docs/COMMERCIAL-CLAIM-AUDIT.md may be used.",
    "claims": [
        {
            "id": cid,
            "category": category,
            "shipped_artifact_evidence": shipped,
            "live_evidence_file": live_file,
            "live_evidence_status": live_status(live_file),
        }
        for cid, category, shipped, live_file in CLAIMS
    ],
}
evidence_files = []
for d in ("evidence/live", "evidence/external"):
    dd = root / d
    if dd.is_dir():
        for f in sorted(dd.glob("*.json")):
            rel = f.relative_to(root).as_posix()
            evidence_files.append({"path": rel, "status": live_status(rel)})
# P0-C TASK 5: these are pointers to evidence stubs, NOT run logs — a
# pointer whose target says NOT_RUN is not a run log. Every pointer's
# target exists in this tree by construction (globbed from disk); the
# status is copied from the target file, never typed here.
manifest["evidence_pointers"] = {
    "note": "Pointers to machine-generated evidence files. Every target "
            "exists in this tree; each status is copied from the target "
            "file by the generator. A NOT_RUN pointer is a stub, not a "
            "result.",
    "files": evidence_files,
}
# run_logs is RESERVED for real test-run logs under evidence/test-runs/.
# It stays [] until such logs actually exist in the tree.
run_logs_dir = root / "evidence" / "test-runs"
run_log_files = []
if run_logs_dir.is_dir():
    for f in sorted(run_logs_dir.rglob("*")):
        if f.is_file():
            run_log_files.append(f.relative_to(root).as_posix())
manifest["run_logs"] = {
    "directory": "evidence/test-runs/",
    "note": "Real test-run logs only. Empty until logs exist in the tree.",
    "files": run_log_files,
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
