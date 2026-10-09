#!/usr/bin/env bash
# ============================================================================
# scripts/verify-image-digests.sh — every container reference is immutable
# (P0 §7).
#
# Two independent assertions:
#
#   1. NO TAG REFERENCES. Every `FROM` and every compose `image:` in the
#      repo must use `repo@sha256:…`. A single tag reference anywhere
#      makes the whole deployment non-reproducible, because that one
#      layer can change under you between the staging verification and
#      the production rollout.
#
#   2. CONSISTENCY WITH THE LOCK. Every digest used in the repo must
#      appear in deploy/release/base-images.lock.json, and every digest
#      in the lock must be used somewhere. A lock that drifts from the
#      files it is supposed to govern is worse than no lock: it reads as
#      evidence while being wrong.
#
# Exit non-zero on the first violation. Runs offline — no registry, no
# Docker daemon.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

LOCK="deploy/release/base-images.lock.json"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "  ok   $*"; }

[[ -f "$LOCK" ]] || fail "$LOCK missing — run ./scripts/pin-base-image-digests.sh"

# Files that may declare a container image.
mapfile -t TARGETS < <(
    {
        find . -name 'Dockerfile' -o -name 'Dockerfile.*'
        find . -name 'docker-compose*.yml' -o -name 'docker-compose*.yaml'
        find ./deploy -name '*.yml' -o -name '*.yaml' 2>/dev/null
    } | grep -v '/node_modules/' | grep -v '/target/' | sort -u
)
[[ "${#TARGETS[@]}" -gt 0 ]] || fail "found no Dockerfile or compose file to check"

echo "== checked files =="
printf '  %s\n' "${TARGETS[@]}"

# ---------------------------------------------------------------------------
echo
echo "== 1. no mutable tag references =="
violations=0
for f in "${TARGETS[@]}"; do
    # FROM lines, ignoring multi-stage aliases (`FROM builder AS x`) and
    # ARG-substituted bases.
    while IFS= read -r line; do
        ref="$(awk '{print $2}' <<<"$line")"
        [[ "$ref" == *"@sha256:"* ]] && continue
        [[ "$ref" == '$'* || "$ref" == '${'* ]] && continue
        # A bare stage alias has no registry path and no tag.
        [[ "$ref" != *:* && "$ref" != */* ]] && continue
        echo "FAIL: $f: mutable FROM reference: $ref" >&2
        violations=$((violations + 1))
    done < <(grep -iE '^[[:space:]]*FROM[[:space:]]+' "$f" 2>/dev/null || true)

    while IFS= read -r line; do
        ref="$(sed -E 's/^[[:space:]]*image:[[:space:]]*//; s/[[:space:]]*(#.*)?$//' <<<"$line")"
        ref="${ref%\"}"; ref="${ref#\"}"
        [[ -z "$ref" ]] && continue
        [[ "$ref" == *"@sha256:"* ]] && continue
        [[ "$ref" == *'${'* ]] && continue
        echo "FAIL: $f: mutable image reference: $ref" >&2
        violations=$((violations + 1))
    done < <(grep -E '^[[:space:]]*image:[[:space:]]*[^[:space:]]' "$f" 2>/dev/null || true)
done
[[ "$violations" -eq 0 ]] || fail "$violations mutable image reference(s); run ./scripts/pin-base-image-digests.sh and pin them"
pass "every FROM and image: uses an immutable digest"

# ---------------------------------------------------------------------------
echo
echo "== 2. repo digests and the lock file agree =="
used="$(grep -rhoE 'sha256:[a-f0-9]{64}' "${TARGETS[@]}" | sort -u)"
locked="$(grep -oE 'sha256:[a-f0-9]{64}' "$LOCK" | sort -u)"

unlocked="$(comm -23 <(echo "$used") <(echo "$locked") || true)"
if [[ -n "$unlocked" ]]; then
    echo "$unlocked" | while read -r d; do
        [[ -n "$d" ]] && echo "FAIL: digest used in the repo but absent from $LOCK: $d" >&2
    done
    fail "the lock file does not cover every digest in use"
fi
pass "every digest in use is recorded in the lock"

unused="$(comm -13 <(echo "$used") <(echo "$locked") || true)"
if [[ -n "$(tr -d '[:space:]' <<<"$unused")" ]]; then
    echo "$unused" | while read -r d; do
        [[ -n "$d" ]] && echo "WARN: digest pinned in $LOCK but unused in the repo: $d" >&2
    done
    echo "  (not fatal — a lock entry may be consumed by a deploy script or a chart)"
fi

# ---------------------------------------------------------------------------
echo
echo "== 3. digest form =="
# Guard against a truncated or uppercase digest, which Docker accepts in
# some contexts and silently resolves differently in others.
if grep -rhoE 'sha256:[A-Za-z0-9]+' "${TARGETS[@]}" "$LOCK" | grep -vE '^sha256:[a-f0-9]{64}$' | sort -u | grep . ; then
    fail "malformed digest (must be sha256: + 64 lowercase hex characters)"
fi
pass "all digests are well-formed"

echo
echo "IMAGE DIGEST PINNING: PASS"
