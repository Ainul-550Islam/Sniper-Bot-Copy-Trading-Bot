#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/buyer-release}"
VERSION="$(cat "$ROOT/VERSION" 2>/dev/null | tr -d ' \n' || echo "0.1.0")"
echo "[build-release-package] sniper-suite $VERSION -> $OUT"
rm -rf "$OUT"
mkdir -p "$OUT"/{source,docs,evidence,sbom,licenses,checksums,manifests}

# Copy source excluding target/node_modules/.git/.env/secrets/private keys/caches/build outputs
# Use rsync if available; fallback to tar pipeline. Both must exclude identical patterns.
if command -v rsync >/dev/null 2>&1; then
  rsync -a --exclude='target/' --exclude='**/target/' --exclude='node_modules/' --exclude='**/node_modules/' --exclude='.git/' --exclude='.env' --exclude='secrets/' --exclude='*.pem' --exclude='*.key' --exclude='__pycache__/' --exclude='.turbo/' --exclude='.next/' --exclude='**/.next/' --exclude='out/' --exclude='**/out/' --exclude='dist/' --exclude='build/' --exclude='.vercel/' --exclude='buyer-release/' --exclude='sbom.json' --exclude='sbom.cyclonedx.json' --exclude='licenses.json' --exclude='licenses.csv' \
    "$ROOT"/ "$OUT/source/"
else
  mkdir -p "$OUT/source"
  # Fallback: use tar pipeline with same excludes
  tar -C "$ROOT" --exclude='target' --exclude='node_modules' --exclude='.git' --exclude='.env' --exclude='secrets' --exclude='buyer-release' --exclude='.next' --exclude='out' --exclude='dist' --exclude='build' --exclude='.turbo' --exclude='sbom.json' --exclude='sbom.cyclonedx.json' --exclude='licenses.json' --exclude='licenses.csv' -cf - . | tar -C "$OUT/source" -xf -
fi
# Prune any accidentally copied excludes (defense in depth)
rm -rf "$OUT/source/target" "$OUT/source/node_modules" "$OUT/source/.git" "$OUT/source/buyer-release" 2>/dev/null || true
rm -rf "$OUT/source/apps/control-plane/.next" "$OUT/source/.next" "$OUT/source/out" "$OUT/source/dist" "$OUT/source/build" "$OUT/source/.turbo" "$OUT/source/.vercel" 2>/dev/null || true
rm -rf "$OUT/source/programs/staking-suite/target" 2>/dev/null || true
rm -f "$OUT/source/sbom.json" "$OUT/source/sbom.cyclonedx.json" "$OUT/source/licenses.json" "$OUT/source/licenses.csv" 2>/dev/null || true
find "$OUT/source" -name ".env" -delete 2>/dev/null || true
find "$OUT/source" -name "*.pem" -o -name "*.key" | xargs rm -f 2>/dev/null || true

# Copy the CONTENTS, never the directory itself: `cp -r "$ROOT/docs" "$OUT/docs"` nests the
# tree one level deeper (buyer-release/docs/docs/...) when $OUT/docs already exists, which
# hides every documented buyer path (`docs/BUYER-TRUTH-REGISTER.md`, `evidence/external/*`).
cp -r "$ROOT/docs/." "$OUT/docs/" 2>/dev/null || true
cp "$ROOT/VERSION" "$OUT/VERSION"
cp "$ROOT/LICENSE" "$OUT/LICENSE" 2>/dev/null || cp "$ROOT/LICENSE.md" "$OUT/LICENSE" 2>/dev/null || echo "SEE LICENSE" > "$OUT/LICENSE"
cp "$ROOT/CHANGELOG.md" "$OUT/CHANGELOG.md" 2>/dev/null || echo "# Changelog" > "$OUT/CHANGELOG.md"
cp "$ROOT/release-manifest.json" "$OUT/manifests/release-manifest.json" 2>/dev/null || echo '{}' > "$OUT/manifests/release-manifest.json"
# release.lock.json is optional: copy it only when the source tree really contains one.
# No `{}` placeholder is shipped — an empty lock file would be a fabricated artifact.
if [ -f "$ROOT/release.lock.json" ]; then
  cp "$ROOT/release.lock.json" "$OUT/manifests/release.lock.json"
else
  echo "[build-release-package] NOTE: release.lock.json absent in source tree — omitted (no placeholder)"
fi
# Evidence + sbom + licenses if generated
# Same rule as docs/: copy the contents so `evidence/external/*.json` lands where the runbook
# and the evidence records say it does.
cp -r "$ROOT/evidence/." "$OUT/evidence/" 2>/dev/null || mkdir -p "$OUT/evidence"
cp "$ROOT/sbom.json" "$OUT/sbom/sbom.json" 2>/dev/null || echo '{"bomFormat":"CycloneDX"}' > "$OUT/sbom/sbom.json"
cp "$ROOT/sbom.cyclonedx.json" "$OUT/sbom/sbom.cyclonedx.json" 2>/dev/null || true
# Supply-chain artifacts live under licenses/ + sbom/ (they are excluded from source/ to avoid
# duplication). The docs promise BOTH formats, so both are copied here and verified below.
cp "$ROOT/licenses.json" "$OUT/licenses/licenses.json" 2>/dev/null || echo '[]' > "$OUT/licenses/licenses.json"
cp "$ROOT/licenses.csv" "$OUT/licenses/licenses.csv" 2>/dev/null || true

# Checksums — SHA256+size+timestamp per artifact independently
CHECKSUM_FILE="$OUT/checksums/SHA256SUMS"
: > "$CHECKSUM_FILE"
echo "# SHA256 checksums for buyer-release artifacts — computed independently per file" >> "$CHECKSUM_FILE"
echo "# generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$CHECKSUM_FILE"
echo "# format: '<sha256>  <relpath>' — verify with: cd <PKG> && sha256sum -c checksums/SHA256SUMS" >> "$CHECKSUM_FILE"
echo "# size/timestamp metadata is on the preceding '#' comment line (coreutils ignores comments)." >> "$CHECKSUM_FILE"
# Enumerate key artifacts
for f in "$OUT/VERSION" "$OUT/LICENSE" "$OUT/CHANGELOG.md" "$OUT/manifests/release-manifest.json" "$OUT/sbom/sbom.json" "$OUT/sbom/sbom.cyclonedx.json" "$OUT/licenses/licenses.json" "$OUT/licenses/licenses.csv"; do
  if [ -f "$f" ]; then
    sha=$(sha256sum "$f" | awk '{print $1}')
    sz=$(wc -c < "$f" | tr -d ' ')
    ts=$(date -u -r "$f" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)
    rel=${f#"$OUT"/}
    # `sha256sum -c` accepts only "<hash>  <path>" lines, so the size/timestamp metadata is
    # written as a `#` comment line above each entry: the documented buyer command
    # `cd <PKG> && sha256sum -c checksums/SHA256SUMS` must really pass (round-5 fix).
    printf "# size=%s ts=%s  %s\n" "$sz" "$ts" "$rel" >> "$CHECKSUM_FILE"
    printf "%s  %s\n" "$sha" "$rel" >> "$CHECKSUM_FILE"
  fi
done
# Also hash every file in source/docs for completeness into checksums/all-files.sha256
# Exclude the checksum file itself: a self-referential entry would record the hash of the empty
# file (it is truncated by the redirection before `find` runs) and could never verify.
# Deliverable source-tree hash: one digest over the shipped source/ tree, so the
# documented buyer check (docs/BUYER-ACCEPTANCE-TEST.md A1, docs/BUYER-REPRODUCTION-GUIDE.md §1)
# has a real value to compare against. The method line below is the exact command the
# buyer runs; scripts/verify-buyer-package.sh recomputes it fail-closed.
if command -v sha256sum >/dev/null 2>&1; then
  TREE_HASH="$(cd "$OUT/source" && find . -type f | sort | xargs sha256sum | sha256sum | awk '{print $1}')"
  TREE_FILES="$(cd "$OUT/source" && find . -type f | wc -l | tr -d ' ')"
  {
    echo "# source-tree digest of the delivered source/ directory"
    echo "# method: cd <PKG>/source && find . -type f | sort | xargs sha256sum | sha256sum"
    echo "# files: $TREE_FILES"
    printf "%s  source/\n" "$TREE_HASH"
  } > "$OUT/checksums/SOURCE-TREE.sha256"
fi
find "$OUT" -type f ! -path "$OUT/checksums/all-files.sha256" -exec sha256sum {} \; | sed "s|$OUT/||" | sort > "$OUT/checksums/all-files.sha256" 2>/dev/null || true


# Verify forbidden not in package
if grep -R "target/" "$OUT/checksums/all-files.sha256" 2>/dev/null | grep -q "target/"; then
  echo "[build-release-package] FAIL: forbidden target/ found in package" >&2
  exit 1
fi
if find "$OUT" -path "*node_modules*" | grep -q .; then
  echo "[build-release-package] FAIL: node_modules found" >&2
  exit 1
fi
if find "$OUT" -path "*/.next/*" -o -path "*/.next" | grep -q .; then
  echo "[build-release-package] FAIL: .next found" >&2
  exit 1
fi
if find "$OUT" -name ".env" | grep -q .; then
  echo "[build-release-package] FAIL: .env found" >&2
  exit 1
fi

# Required packaged artifacts: docs promise both license formats + both SBOM formats.
for req in "sbom/sbom.json" "sbom/sbom.cyclonedx.json" "licenses/licenses.json" "licenses/licenses.csv"; do
  if [ ! -f "$OUT/$req" ]; then
    echo "[build-release-package] FAIL: missing packaged artifact $req" >&2
    exit 1
  fi
done
# all-files.sha256 must never contain a self-referential (empty-file) entry.
if grep -q "checksums/all-files.sha256$" "$OUT/checksums/all-files.sha256" 2>/dev/null; then
  echo "[build-release-package] FAIL: all-files.sha256 contains a self-referential entry" >&2
  exit 1
fi
# Packaged docs must match the manifest docs count (fail-closed when python3 is available).
if command -v python3 >/dev/null 2>&1; then
  PKG_DOCS="$(find "$OUT/docs" -name '*.md' | wc -l | tr -d ' ')"
  MAN_DOCS="$(python3 -c "import json;print(json.load(open('$ROOT/release-manifest.json')).get('docs_files',''))" 2>/dev/null || echo "")"
  if [ -n "$MAN_DOCS" ] && [ "$PKG_DOCS" != "$MAN_DOCS" ]; then
    echo "[build-release-package] FAIL: packaged docs ($PKG_DOCS) != manifest docs_files ($MAN_DOCS)" >&2
    exit 1
  fi
fi

# --- documented package layout (fail-closed) ---------------------------------
# Every path the buyer docs tell the reader to open must exist at package root.
# A nested copy (docs/docs, evidence/evidence) means the builder re-introduced the
# `cp -r <dir> <existing-dir>` nesting defect that hides all of them.
for req in docs/BUYER-TRUTH-REGISTER.md docs/BUYER-EVIDENCE-PACK.md docs/BUYER-HANDOVER-CHECKLIST.md docs/HANDOVER.md docs/BACKUP-RESTORE.md docs/SECURITY.md docs/FINAL-BUYER-GAP-LEDGER.md manifests/release-manifest.json VERSION; do
  if [ ! -f "$OUT/$req" ]; then
    echo "[build-release-package] FAIL: documented package path missing: $req" >&2
    exit 1
  fi
done
if [ -d "$OUT/docs/docs" ] || [ -d "$OUT/evidence/evidence" ]; then
  echo "[build-release-package] FAIL: nested docs/docs or evidence/evidence directory in package" >&2
  exit 1
fi
for ev in billing_stripe custody_vault deployment_deployment funded-preflight_funded solana_solana_rpc staking_staking_validator; do
  if [ ! -f "$OUT/evidence/external/$ev.json" ]; then
    echo "[build-release-package] FAIL: missing evidence/external/$ev.json" >&2
    exit 1
  fi
done
if [ ! -f "$OUT/checksums/SOURCE-TREE.sha256" ]; then
  echo "[build-release-package] FAIL: missing checksums/SOURCE-TREE.sha256" >&2
  exit 1
fi

echo "[build-release-package] OK -> $OUT"
# `head` closes the pipe early, so tolerate SIGPIPE instead of failing the gate
# (`set -o pipefail` would otherwise report exit 141 on a successful build).
ls -R "$OUT" 2>/dev/null | head -n 100 || true
cat "$CHECKSUM_FILE"
