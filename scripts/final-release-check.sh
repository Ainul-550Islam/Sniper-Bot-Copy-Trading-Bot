#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
echo "[final-release-check] sniper-suite — $(cat "$ROOT/VERSION" 2>/dev/null || echo "0.1.0") — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
FAIL=0
pass() { echo "  PASS $*"; }
fail() { echo "  FAIL $*"; FAIL=1; }

# 1. fmt
echo "[1/8] cargo fmt --check"
if cargo fmt --check 2>&1; then pass "fmt"; else fail "fmt"; fi

# 2. cargo check workspace
echo "[2/8] cargo check --workspace"
if cargo check --workspace 2>&1 | tail -n 20; then pass "check"; else fail "check"; fi

# 3. clippy per-crate (workspace clippy may timeout — check sequentially with timeout)
echo "[3/8] cargo clippy (per-crate, -D warnings)"
for crate in sniper-suite saas-sdk bot-core solana-kit module-sniper module-copy module-polymarket module-telegram; do
  echo "  clippy -p $crate"
  if timeout 120 cargo clippy -p "$crate" -- -D warnings 2>&1 | tail -n 5; then pass "clippy $crate"; else fail "clippy $crate"; fi
done

# 4. unit tests (saas-sdk fast; sniper-suite lib tests)
echo "[4/8] cargo test -p saas-sdk"
if cargo test -p saas-sdk 2>&1 | tail -n 20; then pass "test saas-sdk"; else fail "test saas-sdk"; fi

# P0-G (G-1): these harnesses are REQUIRED. A failed, timed-out or unrunnable
# harness is a FAIL, never a PASS. The old fallback that accepted two marker
# files as proof of a test run has been removed. Running them locally needs a
# toolchain and ~8 GB RAM; if that is missing, this step fails and the
# missing run must be recorded as NOT_RUN in the receipts (see CI-HANDOFF.md).
echo "[4b] cargo test -p sniper-suite --test {observability_config,release_manifest_integration,buyer_package_integration,backup_restore_integration} (CARGO_BUILD_JOBS=1)"
if CARGO_BUILD_JOBS=1 cargo test -p sniper-suite --test observability_config --test release_manifest_integration --test buyer_package_integration --test backup_restore_integration 2>&1 | tail -n 20; then
  pass "test sniper-suite integration (4 harnesses)"
else
  fail "test sniper-suite integration (run failed or could not run; NOT a pass)"
fi
# Binary unit tests are required too (P0-G G-1). The previous 30-second
# timeout guaranteed a 'skip' that was never counted as a failure.
echo "[4c] cargo test -p sniper-suite --bins"
if CARGO_BUILD_JOBS=1 cargo test -p sniper-suite --bins 2>&1 | tail -n 20; then
  pass "test sniper-suite binary unit tests"
else
  fail "test sniper-suite binary unit tests (run failed or could not run; NOT a pass)"
fi

# 5. secret scan (same logic as verify-buyer-package: exclude is_secret_like etc)
# 2026-09-24 batch6 docs document hygiene and mention PEM header as check name — exclude those doc references
# 2026-09-24 Batch7 EXTERNAL-VALIDATION-RUNBOOK also documents the policy (BEGIN PRIVATE KEY never in evidence) — exclude that doc reference; redaction helpers contain the literal but also <redacted>
# P0-G (G-1, G-2): ONE secret scanner. The ad-hoc grep chain that used to live
# here disagreed with scripts/scan-secrets.sh (it flagged a detector-list string
# with no key body). It also had a pipefail/grep -q SIGPIPE path that could
# report PASS on real matches. scan-secrets.sh is the single source of truth
# (it covers stripe/AWS/evm/solana/pem-with-body patterns over the whole tree).
echo "[5/8] secret scan (scripts/scan-secrets.sh, single source of truth)"
if bash "$ROOT/scripts/scan-secrets.sh"; then
  pass "secret scan"
else
  fail "secret scan found real private key or token (see findings above; redacted)"
fi

# 6. stale manifest counts
echo "[6/8] release-manifest stale check"
DOCS=$(find docs -type f | wc -l | tr -d ' ')
RUST=$(find crates -name "*.rs" | wc -l | tr -d ' ')
TESTS=$(grep -r "#\[test\]" crates 2>/dev/null | wc -l | tr -d ' ')
MIGS=$(ls -1 crates/core/migrations/*.sql 2>/dev/null | wc -l | tr -d ' ')
echo "  docs=$DOCS rust=$RUST tests=$TESTS migrations=$MIGS"
# Compare to release-manifest.json if exists
if [ -f "$ROOT/release-manifest.json" ]; then
  python3 - "$DOCS" "$RUST" "$TESTS" "$MIGS" "$ROOT/release-manifest.json" << 'PY'
import json, sys
docs, rust, tests, migs, path = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
d=json.loads(open(path).read())
stale=[]
if d.get("docs_files")!=docs: stale.append(f"docs_files {d.get('docs_files')} != {docs}")
if d.get("rust_files")!=rust: stale.append(f"rust_files {d.get('rust_files')} != {rust}")
if d.get("test_count")!=tests: stale.append(f"test_count {d.get('test_count')} != {tests}")
if d.get("migrations")!=migs: stale.append(f"migrations {d.get('migrations')} != {migs}")
if stale:
    print("  STALE: "+" ; ".join(stale))
    sys.exit(1)
else:
    print("  manifest counts ok")
PY
  if [ $? -eq 0 ]; then pass "manifest counts"; else fail "manifest counts stale"; fi
else
  echo "  no release-manifest.json — skip"; pass "manifest skip"
fi

# 6b. migration graph and tenant strategy separation
if bash "$ROOT/scripts/verify-migration-graph.sh"; then
  pass "migration graph"
else
  fail "migration graph"
fi
if bash "$ROOT/tests/release/stats_current.sh"; then
  pass "stats current (docs/STATS.md + markers)"
else
  fail "stats current (run scripts/generate-stats.sh)"
fi
# P0-C TASK 6: every tracked shell script must keep its exec bit.
if bash "$ROOT/scripts/verify-script-modes.sh"; then
  pass "script exec bits (tracked *.sh)"
else
  fail "script exec bits (see offenders above)"
fi
# P0-D TASK 1: high-confidence secret scan over the working tree.
if bash "$ROOT/scripts/scan-secrets.sh"; then
  pass "secret scan (working tree)"
else
  fail "secret scan (findings listed above; rotate anything real)"
fi

# 7. SBOM + license exist and have sha
echo "[7/8] SBOM/license artifacts"
if [ -f "$ROOT/sbom.json" ]; then pass "sbom.json exists"; sha=$(sha256sum "$ROOT/sbom.json" | awk '{print $1}'); echo "    sbom sha256=$sha size=$(wc -c < "$ROOT/sbom.json")"; else fail "sbom.json missing (run scripts/generate-sbom.sh)"; fi
if [ -f "$ROOT/licenses.json" ]; then pass "licenses.json exists"; else fail "licenses.json missing (run scripts/generate-license-report.sh)"; fi
if [ -f "$ROOT/sbom.cyclonedx.json" ]; then pass "sbom.cyclonedx.json exists"; fi

# 8. buyer package (build if not exists, then verify)
echo "[8/8] buyer package"
if [ ! -d "$ROOT/buyer-release" ]; then
  echo "  buyer-release missing — building"
  bash "$ROOT/scripts/build-release-package.sh" "$ROOT/buyer-release" 2>&1 | tail -n 30 || fail "build-release-package"
fi
if bash "$ROOT/scripts/verify-buyer-package.sh" 2>&1 | tee /tmp/verify-buyer.log | tail -n 40; then pass "verify-buyer-package"; else fail "verify-buyer-package"; fi
# verify-delivery hygiene fails locally due to target/ (CI is clean). Allow target-only hygiene as WARN.
# P0-G (G-1): a target/ directory in the tree is a real hygiene violation. The
# previous branch turned it into a PASS ("6/7 PASS"). It is now a FAIL: remove
# target/ from the tree before packaging (it is never part of the buyer ZIP).
if bash "$ROOT/scripts/verify-delivery.sh" 2>&1 | tee /tmp/verify-delivery.log | tail -n 40; then
  pass "verify-delivery"
else
  fail "verify-delivery (see /tmp/verify-delivery.log)"
fi

echo ""
if [ $FAIL -eq 0 ]; then
  echo "[final-release-check] ALL PASS"
else
  echo "[final-release-check] SOME CHECKS FAILED — see FAIL above"
fi
exit $FAIL
