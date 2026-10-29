#!/usr/bin/env bash
# ============================================================================
# regen-snapshot-manifest.sh — rebuild SNAPSHOT-MANIFEST.sha256.
#
# The manifest is a point-in-time integrity record of the TRACKED product
# tree: one `sha256sum` line per file, ordered by path. It is regenerated
# (not hand-edited) whenever the tracked file set changes, so it can never
# again name a file that has been deleted or untracked — the fate of the
# previous copy, which still listed `sbom.cyclonedx.json`, `rustup-init.sh`
# and the AUDIT-* agent dumps long after they had been removed.
#
# The exclusion rule is the same one scripts/verify-delivery.sh enforces for
# tracked junk, plus the manifest itself and the generated SHA256SUMS: a
# checksum file cannot contain its own checksum.
#
# Usage:   scripts/regen-snapshot-manifest.sh
# Exit:    0 on success; 1 if not inside a git work tree or the tree is empty.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "regen-snapshot-manifest: FAIL — not inside a git work tree" >&2
    exit 1
fi

OUT="SNAPSHOT-MANIFEST.sha256"

# Junk paths/scripts/verify-delivery.sh already rejects, plus generated
# artifacts that must not be checksummed into the record.
EXCLUDE_RE='\.cargo/bin|\.cargo/env|rustup-init|^\.config/|^uploads/|P0-D-FULL-CODE|test-results|^AUDIT|\.out$|^buyer-release/|^SHA256SUMS$|^SNAPSHOT-MANIFEST\.sha256$'

tmp="$(mktemp "${TMPDIR:-/tmp}/snapshot-manifest.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

# -z throughout: several paths in this tree contain spaces.
if ! git ls-files -z | grep -zvE "$EXCLUDE_RE" | LC_ALL=C sort -z | xargs -0 -r sha256sum >"$tmp"; then
    echo "regen-snapshot-manifest: FAIL — could not checksum the tracked tree" >&2
    exit 1
fi

count="$(wc -l <"$tmp" | tr -d ' ')"
if [ "$count" -eq 0 ]; then
    echo "regen-snapshot-manifest: FAIL — tracked tree is empty" >&2
    exit 1
fi

mv "$tmp" "$OUT"
trap - EXIT
echo "regen-snapshot-manifest: wrote $OUT ($count files)"
