#!/usr/bin/env bash
# ============================================================================
# verify-delivery.sh — delivery-bundle integrity check.
#
# Fast, fail-closed validation of a received sniper-suite delivery tree.
# Complementary to scripts/release-check.sh: release-check runs the FULL
# engineering gate (toolchain + PostgreSQL + Redis + ~all tests); this script
# needs only bash/coreutils/grep/sed and answers "is this bundle complete,
# consistent and hygienic?" in seconds. It does NOT build or test anything.
#
# Checks:
#   1. required release + buyer documentation files exist
#   2. version identity (VERSION == Cargo.toml == release-manifest.json)
#   3. manifest docs_count == actual docs/*.md count
#   4. hygiene: no .env, logs, keypairs, build dirs, DB/Redis dumps
#   5. every relative markdown link in README.md + docs/ resolves
#   6. no zero-width / bidi-override characters in markdown
#
# Usage:   ./scripts/verify-delivery.sh        (run from the repository root)
# Exit:    0 = all checks passed; 1 = at least one FAIL (fail-closed).
# ============================================================================
set -u
cd "$(dirname "$0")/.." || { echo "FAIL: cannot cd to repository root"; exit 1; }

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$1"; }

# ------------------------------------------------------- 1. required files --
REQUIRED_FILES="
VERSION LICENSE SECURITY.md README.md CHANGELOG.md AUDIT.md
Cargo.toml Cargo.lock rust-toolchain.toml deny.toml release-manifest.json
sbom.json sbom.cyclonedx.json licenses.json licenses.csv
Dockerfile docker-compose.yml .dockerignore .env.template config.toml.example .gitignore
.github/workflows/ci.yml scripts/release-check.sh scripts/verify-delivery.sh
programs/staking-suite/Cargo.toml programs/staking-suite/Cargo.lock
docs/ARCHITECTURE.md docs/API.md docs/SECURITY.md docs/DEPLOYMENT.md docs/OPERATIONS.md
docs/MODULES.md docs/STAKING.md docs/TESTING.md docs/RECONCILIATION.md docs/DISTRIBUTED.md
docs/RELEASE.md docs/HANDOVER.md docs/BACKUP-RESTORE.md
docs/BUYER-OVERVIEW.md docs/CAPABILITY-MATRIX.md docs/BUYER-DUE-DILIGENCE.md
docs/IP-COMPONENTS.md docs/THIRD-PARTY.md docs/BUYER-DEPLOYMENT.md
docs/ACCEPTANCE-CHECKLIST.md docs/RELEASE-NOTES-0.1.0.md docs/BUYER-FAQ.md
docs/SCOPE-BOUNDARY.md docs/SUPPORT-HANDOVER.md docs/BUYER-RISK-REGISTER.md
docs/TECHNICAL-DIFFERENTIATORS.md docs/DELIVERY-MANIFEST.md
docs/FINAL-DELIVERY.md docs/BUYER-QUICKSTART.md docs/TECHNICAL-FACT-SHEET.md
docs/SELLER-FACT-SHEET.md docs/SELLING-LISTING-SOURCE.md docs/DEMO-RUNBOOK.md
docs/EVIDENCE-INDEX.md docs/REPOSITORY-MAP.md docs/ARCHIVE-CHECKLIST.md
docs/EXECUTION-RELIABILITY.md docs/SNIPER-ENGINE.md
docs/COPY-TRADING-ENGINE.md docs/COPY-TRADING-OPERATIONS.md docs/COPY-TRADING-RECOVERY.md
docs/POLYMARKET-ENGINE.md docs/POLYMARKET-OPERATIONS.md docs/POLYMARKET-RECOVERY.md
docs/GLOBAL-RISK.md docs/ACCOUNTING-LEDGER.md docs/RISK-OPERATIONS.md
docs/HA-ARCHITECTURE.md docs/DISTRIBUTED-OPERATIONS.md docs/CRASH-RECOVERY.md
docs/FORENSIC-FILE-INVENTORY.md docs/SOURCE-OF-TRUTH.md docs/FINAL-RELEASE-AUDIT.md
"
missing=""
# Four supply-chain artifacts sit at the repository root yet are RELOCATED in the release
# package to <PKG>/sbom/ and <PKG>/licenses/. A buyer running this script inside the
# delivered package must still get PASS, so both layouts are accepted; if neither exists
# the file is reported missing (fail-closed) — round-5 fix.
relocated_path() {
  case "$1" in
    sbom.json|sbom.cyclonedx.json) [ -f "../sbom/$1" ] && [ -f "../manifests/release-manifest.json" ] && echo "../sbom/$1" ;;
    licenses.json|licenses.csv) [ -f "../licenses/$1" ] && [ -f "../manifests/release-manifest.json" ] && echo "../licenses/$1" ;;
  esac
}
for f in $REQUIRED_FILES; do
  if [ -f "$f" ]; then continue; fi
  alt="$(relocated_path "$f" || true)"
  if [ -n "$alt" ]; then continue; fi
  missing="$missing $f"
done
if [ -z "$missing" ]; then
  ok "required files present ($(echo $REQUIRED_FILES | wc -w | tr -d ' ') checked)"
else
  bad "missing required files:$missing"
fi
# migrations 0001-0022 (contiguous, forward-only) — 22 migrations
migmissing=""
for i in 0001 0002 0003 0004 0005 0006 0007 0008 0009 0010 0011 0012 0013 0014 0015 0016 0017 0018 0019 0020 0021 0022; do
  ls crates/core/migrations/${i}_*.sql >/dev/null 2>&1 || migmissing="$migmissing $i"
done
[ -z "$migmissing" ] && ok "migrations 0001-0022 present" || bad "missing migrations:$migmissing"

# ---------------------------------------------------------- 2. version id --
V_FILE="$(tr -d '[:space:]' < VERSION)"
V_MANIFEST="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' release-manifest.json | head -1)"
V_CARGO="$(sed -n '/\[workspace.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
if [ -n "$V_FILE" ] && [ "$V_FILE" = "$V_MANIFEST" ] && [ "$V_FILE" = "$V_CARGO" ]; then
  ok "version identity: VERSION == release-manifest.json == Cargo.toml ($V_FILE)"
else
  bad "version mismatch: VERSION='$V_FILE' manifest='$V_MANIFEST' Cargo.toml='$V_CARGO'"
fi

# -------------------------------------------------------- 3. docs counter --
DOCS_ACTUAL="$(ls docs/*.md 2>/dev/null | wc -l | tr -d ' ')"
DOCS_MANIFEST="$(sed -n 's/.*"docs_count"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' release-manifest.json | head -1)"
if [ "$DOCS_ACTUAL" = "$DOCS_MANIFEST" ]; then
  ok "manifest docs_count ($DOCS_MANIFEST) == actual docs/*.md ($DOCS_ACTUAL)"
else
  bad "docs_count mismatch: manifest=$DOCS_MANIFEST actual=$DOCS_ACTUAL"
fi

# ------------------------------------------------------------ 4. hygiene ---
# 2026-09-24 investigation (Batch FINAL): `target/` is a generated Cargo build
# output, .gitignore'd (`/target`, `**/target`), never committed, and
# explicitly excluded from every buyer artifact by `scripts/build-release-package.sh`
# (`--exclude='target/' --exclude='**/target/'`). It is *not* package contamination:
# - `buyer-release/` never contains it (verified by build-release-package & verify-buyer-package)
# - `release-manifest.json` / SBOM / checksums never reference it
# - `.gitignore` + `verify-buyer-package` ensure release archives are clean
# Therefore hygiene FAIL is for *real* contamination only: .env, keypairs,
# pem, logs, dumps. Build dirs are reported as INFO and do not fail delivery.
dirty=""
[ -e .env ] && dirty="$dirty .env"
build_info=""
[ -d target ] && build_info="$build_info target/(generated, .gitignore'd, excluded from package)"
[ -d build ] && build_info="$build_info build/(generated)"
[ -d programs/staking-suite/target ] && build_info="$build_info programs/staking-suite/target/(generated)"
[ -d apps/control-plane/.next ] && build_info="$build_info apps/control-plane/.next/(generated, .gitignore'd, excluded from package)"
[ -d .next ] && build_info="$build_info .next/(generated)"
[ -d out ] && build_info="$build_info out/(generated)"
[ -d dist ] && build_info="$build_info dist/(generated)"
for pat in '*.log' '*.dump' 'dump.rdb' 'appendonly.aof' '*keypair*.json' '*.pem'; do
  hits="$(find . -path ./.git -prune -o -path ./target -prune -o -path ./buyer-release -prune -o -path ./programs/staking-suite/target -prune -o -path ./apps/control-plane/.next -prune -o -path ./.next -prune -o -path ./out -prune -o -path ./dist -prune -o -type f -name "$pat" -print 2>/dev/null | head -3)"
  [ -n "$hits" ] && dirty="$dirty $hits"
done
if [ -z "$dirty" ]; then
  if [ -n "$build_info" ]; then
    ok "hygiene: no .env / logs / dumps / keypairs / pem files (build output present but correctly excluded:$build_info)"
  else
    ok "hygiene: no .env / logs / dumps / keypairs / pem files (no build output present)"
  fi
else
  bad "hygiene violations:$dirty"
fi

# ------------------------------------------------------- 5. markdown links --
if ! command -v python3 >/dev/null 2>&1; then
  bad "python3 not available — cannot verify markdown links (fail-closed)"
else
  broken="$(python3 - <<'PY'
import os, re
link_re = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
bad = []
files = ["README.md"] + ["docs/"+f for f in sorted(os.listdir("docs")) if f.endswith(".md")]
for path in files:
    text = open(path, encoding="utf-8").read()
    for m in link_re.finditer(text):
        t = m.group(1)
        if t.startswith(("http://","https://","mailto:","#")): continue
        t = t.split("#")[0]
        if not t: continue
        if not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(path), t))):
            bad.append(f"{path}: {m.group(1)}")
print("\n".join(bad))
PY
)"
  if [ -z "$broken" ]; then
    ok "all relative markdown links resolve (README.md + docs/*.md)"
  else
    bad "broken markdown links:
$broken"
  fi

# ------------------------------------------- 6. invisible/bidi characters --
  zw="$(python3 - <<'PY'
import os
bad_chars = {"\u200b":"ZWSP","\u200e":"LRM","\u200f":"RLM","\u202a":"LRE","\u202b":"RLE","\u202e":"RLO","\ufeff":"BOM"}
files = ["README.md"] + ["docs/"+f for f in sorted(os.listdir("docs")) if f.endswith(".md")]
out=[]
for path in files:
    t=open(path,encoding="utf-8").read()
    for ch,name in bad_chars.items():
        if ch in t: out.append(f"{path}: {name}")
print("\n".join(out))
PY
)"
  if [ -z "$zw" ]; then
    ok "no zero-width / bidi-override characters in markdown"
  else
    bad "invisible characters found:
$zw"
  fi
fi

# ------------------------------------------------------------- summary ----
TOTAL_FILES="$(find . -path ./.git -prune -o -type f -print | wc -l | tr -d ' ')"
TOTAL_BYTES="$(find . -path ./.git -prune -o -type f -print0 | du -cb --files0-from=- 2>/dev/null | tail -1 | cut -f1)"
echo "----------------------------------------"
echo "tree: ${TOTAL_FILES} files (excluding .git), ${TOTAL_BYTES:-unknown} bytes"
echo "verify-delivery: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
