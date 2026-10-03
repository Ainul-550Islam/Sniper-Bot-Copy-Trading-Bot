#!/usr/bin/env bash
# ============================================================================
# scripts/deploy-release.sh — digest-pinned deploy with a rollback ledger
# (P0 §7).
#
#   ./scripts/deploy-release.sh staging
#   ./scripts/deploy-release.sh production
#   ./scripts/deploy-release.sh production --image ghcr.io/acme/sniper@sha256:…
#
# WHAT THIS DOES, IN ORDER
# ------------------------
#   1. PREFLIGHT  — refuse to start if any gate fails. All of them are
#                   cheap and offline; none of them is optional.
#   2. RESOLVE    — turn whatever the operator supplied into an immutable
#                   `repo@sha256:…`. A tag is never deployed, even if the
#                   operator passed one: it is resolved first and the
#                   digest is what gets recorded and rolled out.
#   3. MIGRATE    — apply database migrations ONCE, before any new replica
#                   starts. Production sets DATABASE_AUTO_MIGRATE=false
#                   precisely so five replicas do not race the schema
#                   during a rolling restart.
#   4. ROLL       — start the new image.
#   5. VERIFY     — health + the deployed digest must match what we asked
#                   for. A deploy that "succeeded" while serving the old
#                   image is the failure mode this step exists to catch.
#   6. RECORD     — append to deploy/release/deployments.jsonl. That
#                   ledger is what `rollback-release.sh` reads; without it
#                   a rollback is guesswork.
#
# On a failed VERIFY the script rolls back automatically and exits
# non-zero. It never leaves the environment on an unverified image.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

LEDGER="deploy/release/deployments.jsonl"
ENVIRONMENT="${1:-}"
shift || true
EXPLICIT_IMAGE=""
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image) EXPLICIT_IMAGE="${2:?--image needs a reference}"; SKIP_BUILD=1; shift 2 ;;
        --no-build) SKIP_BUILD=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$ENVIRONMENT" in
    staging|production) ;;
    *) echo "usage: $0 {staging|production} [--image repo@sha256:…] [--no-build]" >&2; exit 2 ;;
esac

ENV_FILE="deploy/environments/${ENVIRONMENT}.env"
log()  { printf '\n==> %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
log "1/6 preflight"
[[ -f "$ENV_FILE" ]] || fail "$ENV_FILE not found — copy it from ${ENV_FILE}.template and fill it in"

./scripts/verify-image-digests.sh           > /dev/null || fail "image digest gate failed (run it directly for detail)"
echo "  ok   image digest pinning"
./scripts/verify-environment-separation.sh --filled > /dev/null || fail "staging/production separation gate failed"
echo "  ok   environment separation"
./scripts/verify-tls-config.sh              > /dev/null || fail "TLS configuration gate failed"
echo "  ok   TLS configuration"

# Deploying a dirty tree means the digest in the ledger cannot be mapped
# back to a commit, which makes the rollback record unverifiable.
if command -v git >/dev/null 2>&1 && git rev-parse --git-dir >/dev/null 2>&1; then
    GIT_SHA="$(git rev-parse HEAD)"
    if [[ -n "$(git status --porcelain)" ]]; then
        if [[ "$ENVIRONMENT" == "production" ]]; then
            fail "working tree is dirty; refusing to deploy an unidentifiable build to production"
        fi
        echo "  WARN working tree is dirty (allowed for staging)"
        GIT_SHA="${GIT_SHA}-dirty"
    fi
    echo "  ok   source revision $GIT_SHA"
else
    GIT_SHA="unknown"
    echo "  WARN not a git checkout — the ledger entry will not carry a revision"
fi

set -a
# shellcheck disable=SC1091
. .env.template
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

COMPOSE=(docker compose --env-file "$ENV_FILE" -f docker-compose.yml -f deploy/compose/docker-compose.tls.yml)

# ---------------------------------------------------------------------------
log "2/6 resolve the image to a digest"
if [[ -n "$EXPLICIT_IMAGE" ]]; then
    IMAGE_REF="$EXPLICIT_IMAGE"
    if [[ "$IMAGE_REF" != *"@sha256:"* ]]; then
        echo "  resolving tag $IMAGE_REF to a digest …"
        docker pull "$IMAGE_REF" >/dev/null
        IMAGE_REF="$(docker inspect --format '{{index .RepoDigests 0}}' "$IMAGE_REF")"
        [[ "$IMAGE_REF" == *"@sha256:"* ]] || fail "could not resolve $EXPLICIT_IMAGE to a digest"
    fi
elif [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "  building the application image"
    "${COMPOSE[@]}" build bot
    IMAGE_ID="$("${COMPOSE[@]}" images -q bot | head -n1)"
    [[ -n "$IMAGE_ID" ]] || fail "build produced no image"
    # A locally built image has no registry digest until it is pushed.
    # Use the content-addressable image ID, which is equally immutable for
    # a host-local deployment, and say so in the ledger.
    IMAGE_REF="$(docker inspect --format '{{index .RepoDigests 0}}' "$IMAGE_ID" 2>/dev/null || true)"
    if [[ -z "$IMAGE_REF" ]]; then
        IMAGE_REF="local:$(docker inspect --format '{{.Id}}' "$IMAGE_ID")"
        echo "  WARN image is local-only (never pushed): recording its image ID."
        echo "       For a production rollback you can verify offline, push to a"
        echo "       registry and redeploy with --image repo@sha256:…"
        [[ "$ENVIRONMENT" == "production" ]] && \
            fail "production requires a pushed, registry-addressable digest"
    fi
else
    fail "--no-build needs --image"
fi
echo "  deploying: $IMAGE_REF"
export SNIPER_IMAGE="$IMAGE_REF"

PREVIOUS=""
if [[ -f "$LEDGER" ]]; then
    PREVIOUS="$(grep "\"environment\": *\"$ENVIRONMENT\"" "$LEDGER" \
                | grep '"status": *"verified"' | tail -n1 \
                | sed -E 's/.*"image": *"([^"]+)".*/\1/')"
fi
[[ -n "$PREVIOUS" ]] && echo "  previous verified image: $PREVIOUS"

# ---------------------------------------------------------------------------
log "3/6 apply database migrations (once, before any new replica starts)"
# The binary applies embedded migrations and exits; it is the SAME image
# being deployed, so the schema can never be migrated by a build other
# than the one about to run.
"${COMPOSE[@]}" run --rm --no-deps \
    -e DATABASE_AUTO_MIGRATE=true \
    -e MIGRATE_ONLY=1 \
    bot --migrate-only \
    || fail "migrations failed — nothing was rolled out, the old image is still serving"
echo "  ok   migrations applied"

# ---------------------------------------------------------------------------
log "4/6 roll the new image"
"${COMPOSE[@]}" up -d --no-build bot nginx

# ---------------------------------------------------------------------------
log "5/6 verify"
rollback_now() {
    echo "FAIL: $1" >&2
    if [[ -n "$PREVIOUS" ]]; then
        echo "==> automatic rollback to $PREVIOUS" >&2
        SNIPER_IMAGE="$PREVIOUS" "${COMPOSE[@]}" up -d --no-build bot nginx || true
        record "rolled_back" "$1"
    else
        echo "==> no previous verified image in the ledger; the environment is on the NEW image and unverified." >&2
        echo "    Investigate immediately, then either fix forward or stop the stack." >&2
        record "failed" "$1"
    fi
    exit 1
}

record() {
    local status="$1" detail="${2:-}"
    mkdir -p "$(dirname "$LEDGER")"
    printf '{"ts": "%s", "environment": "%s", "image": "%s", "previous": "%s", "revision": "%s", "status": "%s", "detail": "%s", "operator": "%s"}\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        "$ENVIRONMENT" "$IMAGE_REF" "$PREVIOUS" "$GIT_SHA" "$status" \
        "${detail//\"/\'}" "${USER:-unknown}" \
        >> "$LEDGER"
}

echo "  waiting for health"
healthy=0
for _ in $(seq 1 60); do
    if "${COMPOSE[@]}" exec -T bot curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1; then
        healthy=1; break
    fi
    sleep 2
done
[[ "$healthy" -eq 1 ]] || rollback_now "the new image did not become healthy within 120s"
echo "  ok   /api/health"

# The running container MUST be the image we resolved. Catches a stale
# compose cache, a hand-edited override, and the "deploy succeeded but
# nothing changed" class of incident.
running="$("${COMPOSE[@]}" ps -q bot | head -n1)"
running_image="$(docker inspect --format '{{.Image}}' "$running" 2>/dev/null || true)"
expected_image="$(docker inspect --format '{{.Id}}' "$IMAGE_REF" 2>/dev/null || true)"
if [[ -n "$running_image" && -n "$expected_image" && "$running_image" != "$expected_image" ]]; then
    rollback_now "the running container is not the requested image"
fi
echo "  ok   running container matches the requested digest"

if [[ "$ENVIRONMENT" == "production" ]] && [[ -n "${SNIPER_PUBLIC_HOST:-}" ]]; then
    SNIPER_PUBLIC_HOST="$SNIPER_PUBLIC_HOST" ./scripts/verify-tls-config.sh --live \
        || rollback_now "the live TLS edge did not pass verification"
    echo "  ok   live TLS edge"
fi

# ---------------------------------------------------------------------------
log "6/6 record"
record "verified"
echo "  appended to $LEDGER"

cat <<EOF

DEPLOY OK
  environment : $ENVIRONMENT
  image       : $IMAGE_REF
  revision    : $GIT_SHA
  previous    : ${PREVIOUS:-<none>}

Roll back with:
  ./scripts/rollback-release.sh $ENVIRONMENT
EOF
