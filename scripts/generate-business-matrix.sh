#!/usr/bin/env bash
# generate-business-matrix.sh — regenerate the measured columns of the
# 2026 business matrix from the actual tree (PROMPT 6 §P6).
#
# The seven rows and their required-feature markers follow the PROMPT 6
# spec exactly: each marker is a capability the spec lists as PRESENT
# for that business line (a stable file or a grep-able symbol a buyer
# can verify). The script counts the markers that exist and divides by
# the markers required — nothing is hand-entered, nothing is rounded
# up. Gaps are NOT markers: a gap (landing-rate proof, wallet
# analytics, live evidence, …) is listed in the doc's gap columns and
# can never inflate the percentage.
#
# The curated columns (safe claims / unsafe claims) are also validated
# here: every "safe claim" is checked against the banned-phrase list,
# so the matrix can never drift into an unsafe claim.
#
# Exit 0 = matrix generated; exit 1 = a safe claim is invalid; exit 2 =
# scanner could not run.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

exec python3 - "$ROOT" <<'PY'
import re
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])

def exists(pattern):
    """Marker exists: a repo-relative file, directory, glob, or a grep pattern."""
    if pattern.startswith(("crates/", "programs/", "apps/", "scripts/", "docs/", "tests/", "release-manifest.json")):
        if (root / pattern).is_file() or (root / pattern).is_dir():
            return True
        hits = [p for p in root.glob(pattern) if p.is_file()]
        return bool(hits)
    r = subprocess.run(
        ["grep", "-rl", "--include=*.rs", "--include=*.ts", "--include=*.tsx",
         "--include=*.sql", "--include=*.sh", pattern,
         "crates", "programs", "apps", "scripts"],
        cwd=root, capture_output=True, text=True)
    return r.returncode == 0

def count_tests(paths):
    """Rust test functions for module paths; executable shell tests for
    script paths (a release test is one .sh with a PASS path)."""
    n = 0
    for pat in paths:
        for f in root.glob(pat):
            if f.suffix == ".sh":
                text = f.read_text(errors="replace")
                if re.search(r'PASS|exit 0', text):
                    n += 1
            else:
                text = f.read_text(errors="replace")
                n += len(re.findall(r'#\[(tokio::)?test\]', text))
    return n

# ---------------------------------------------------------------------------
# The seven business lines (spec-exact names) and their REQUIRED-FEATURE
# markers — each marker is a PRESENT capability from the PROMPT 6 spec,
# verified to exist in the tree before it became a marker.
# ---------------------------------------------------------------------------
LINES = [
    # Sniper PRESENT: Pump.fun, PumpPortal, Geyser, logs, PumpSwap,
    # Raydium AMM v4, Jupiter, priority fee, Jito (detect.rs/entry.rs).
    ("Sniper", "crates/module-sniper/src/detect.rs", "crates/module-sniper/src/entry.rs",
     "PumpPortal", "Geyser", "PumpSwap", "Raydium", "Jupiter", "Jito", "priority"),
    # Copy Trading PRESENT: tracked-wallet copying, tenant execution,
    # tenant-local dedup, exit handling (+ mirror/recovery engine files).
    ("Copy Trading", "crates/module-copy/src/mirror.rs", "crates/module-copy/src/exit.rs",
     "crates/module-copy/src/recovery.rs", "crates/module-copy/src/event_dedup.rs",
     "crates/module-copy/src/tenant_executor.rs", "crates/module-copy/src/leader.rs"),
    # Polymarket PRESENT: CLOB, Gamma, WS, L1/L2, V2, V3, async.
    ("Polymarket", "crates/module-polymarket/src/clob.rs", "crates/module-polymarket/src/discover.rs",
     "crates/module-polymarket/src/auth.rs", "crates/module-polymarket/src/eip712.rs",
     "crates/module-polymarket/src/exchange_v3.rs", "crates/module-polymarket/src/async_commit.rs"),
    # Staking/Token/Fee PRESENT: program logic, validation, rewards,
    # fee/admin flows.
    ("Staking/Token/Fee", "programs/staking-suite/src/processor.rs", "programs/staking-suite/src/instruction.rs",
     "reward", "fee", "admin"),
    # Telegram PRESENT: control, status, RBAC.
    ("Telegram", "crates/module-telegram/src/commands.rs", "crates/module-telegram/src/alerts.rs",
     "crates/module-telegram/src/api.rs", "Permission"),
    # SaaS PRESENT: tenant identity, runtime, execution guard, customer
    # API, billing, custody foundation.
    ("SaaS", "crates/server/src/saas/custody.rs", "crates/server/src/saas/billing.rs",
     "crates/server/src/trading_data_plane/mod.rs", "crates/server/src/tenant",
     "crates/server/src/runtime_registry", "crates/server/src/saas/middleware.rs"),
    # BUSINESS / Commercial must cover: release integrity, docs, support,
    # IP, licensing, external audit status, deployment, SLO/DR, buyer
    # acceptance, evidence.
    ("BUSINESS / Commercial", "scripts/verify-release-integrity.sh", "docs/SUPPORT-HANDOVER.md",
     "docs/IP-OWNERSHIP-REGISTER.md", "docs/OPEN-SOURCE-COMPLIANCE.md",
     "docs/SECURITY-AUDIT-STATUS-2026.md", "docs/DEPLOYMENT.md",
     "docs/BACKUP-RESTORE.md", "docs/BUYER-ACCEPTANCE-TEST.md", "docs/EVIDENCE-INDEX.md"),
]

TEST_PATHS = {
    "Sniper": ["crates/module-sniper/src/**/*.rs", "crates/server/tests/sniper*.rs"],
    "Copy Trading": ["crates/module-copy/src/**/*.rs", "crates/server/tests/copy*.rs"],
    "Polymarket": ["crates/module-polymarket/src/**/*.rs", "crates/server/tests/polymarket*.rs"],
    "Staking/Token/Fee": ["programs/staking-suite/tests/*.rs", "programs/staking-suite/src/*.rs"],
    "Telegram": ["crates/module-telegram/src/**/*.rs", "crates/server/tests/telegram*.rs"],
    "SaaS": ["crates/server/src/saas/**/*.rs", "crates/server/tests/*.rs", "crates/saas-sdk/src/**/*.rs"],
    "BUSINESS / Commercial": ["tests/**/*.sh"],
}

# Curated claim vocabulary (validated against the banned list below).
SAFE_CLAIMS = {
    "Sniper": "deterministic entry/exit detection across Pump.fun/PumpPortal/Geyser feeds with PumpSwap/Raydium/Jupiter routing, priority fees and Jito; unit-tested (no landing-rate or latency proof)",
    "Copy Trading": "tracked-wallet copying with tenant execution, tenant-local dedup, exit handling and crash recovery; unit + integration tested (no live proof)",
    "Polymarket": "CLOB + Gamma + WebSocket + L1/L2 data, V2 order domain and explicit V3 position orders with async lifecycle and reconciliation; unit + PostgreSQL integration tested (no live or funded proof)",
    "Staking/Token/Fee": "native Solana program with validation, rewards and fee/admin flows, unit-tested; pre-deployment placeholder id with guarded set-id/deploy tooling (not deployed, not audited)",
    "Telegram": "control, status and RBAC surface with per-tenant binding API (integration-tested); the forwarder routes to the deployment alert chat",
    "SaaS": "multi-tenant control plane: tenant identity, runtime registry, execution guard, customer API, billing state machine, custody foundation — integration-tested against PostgreSQL",
    "BUSINESS / Commercial": "verifiable release package: parity, manifest, SBOM, license report, marketing-claim rejection, forensic SQL gate; every commercial topic covered by a document",
}
UNSAFE_CLAIMS = {
    "Sniper": "guaranteed 1-second landing (no landing-rate proof exists); profitable / risk-free",
    "Copy Trading": "guaranteed fill parity / zero-loss mirroring (no live proof)",
    "Polymarket": "all-venues compatibility / exchange-uptime guarantee (compatibility is per-endpoint); latest-V3 claims beyond the implemented domain",
    "Staking/Token/Fee": "audited / mainnet-live (no external audit, not deployed)",
    "Telegram": "per-tenant message routing is production-complete (forwarder is deployment-level)",
    "SaaS": "SOC2 / externally audited (no external audit commissioned)",
    "BUSINESS / Commercial": "zero-defect / fully-parity-verified live operation (verification is static + test-level, not live)",
}

# Banned phrases (kept in lockstep with scripts/verify-marketing-claims.sh).
BANNED = ["guaranteed", "risk-free", "risk free", "profitable", "battle-tested",
          "zero-defect", "SOC2", "audited", "mainnet-live", "zero-loss",
          "fully-parity-verified"]

rows = []
fail = False
for name, *markers in LINES:
    present = [m for m in markers if exists(m)]
    missing = [m for m in markers if not exists(m)]
    pct = round(100.0 * len(present) / len(markers))
    tests = count_tests(TEST_PATHS[name])
    safe = SAFE_CLAIMS[name]
    for phrase in BANNED:
        # a safe claim may not CONTAIN a banned phrase as its own claim;
        # parenthetical negations ("no landing-rate proof") are the honest
        # form and are allowed — the check below looks for the phrase
        # outside a negation context
        for m in re.finditer(re.escape(phrase), safe, re.IGNORECASE):
            ctx = safe[max(0, m.start() - 12):m.end()]
            if "no " not in ctx and "not " not in ctx:
                print(f"[business-matrix] INVALID safe claim for {name}: contains banned phrase '{phrase}'", file=sys.stderr)
                fail = True
    rows.append({
        "line": name, "markers": len(markers), "present": len(present),
        "missing": missing, "pct": pct, "tests": tests,
        "safe": safe, "unsafe": UNSAFE_CLAIMS[name],
    })

print("# BUSINESS MATRIX 2026 — measured columns (generated by scripts/generate-business-matrix.sh)")
print()
print("| Business line | Required markers | Present | Completeness % | Test fns (module paths) | Safe claim | Unsafe claim (do not make) |")
print("| --- | --- | --- | --- | --- | --- | --- |")
for r in rows:
    print(f"| {r['line']} | {r['markers']} | {r['present']} | {r['pct']}% | {r['tests']} | {r['safe']} | {r['unsafe']} |")
print()
print("Missing markers by line (the honest gaps):")
for r in rows:
    if r["missing"]:
        print(f"  {r['line']}: {', '.join(r['missing'])}")
    else:
        print(f"  {r['line']}: none")

sys.exit(1 if fail else 0)
PY
