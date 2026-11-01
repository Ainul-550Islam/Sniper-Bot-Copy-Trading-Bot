#!/usr/bin/env bash
# Container-asset verification against REAL third-party tools and the LIVE registry.
#
# What this gate does when the tools are present:
#   1. hadolint on every Dockerfile (a real Dockerfile linter, not our own regexes)
#   2. docker compose `config -q` on the base file and on every documented
#      overlay chain (real Compose parser + interpolation, no daemon needed)
#   3. scripts/verify-image-digests-live.py — resolves every pinned image digest and
#      tag against registry-1.docker.io and asserts the registry returns the
#      same content digest plus a linux/amd64 platform entry
#
# What this gate does NOT do: build an image or run a container. There is no
# container runtime in the verification environment; that is recorded as a
# limitation, never as a pass.
#
# Degradation: if a tool or the network is missing, the section prints SKIPPED
# with the reason and the gate still exits 0 — the same pattern as
# helm_image_contract.sh, so CI without these tools is not a false failure.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

FAILED=0

# The delivered ZIP has no .git, so a buyer running this gate inside the
# extracted package must still find the files: fall back to `find`.
list_files() {
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git ls-files
  else
    find . -type f \
      -not -path './.git/*' -not -path './target/*' -not -path './node_modules/*' \
      -not -path './buyer-release/*' -not -path './apps/control-plane/node_modules/*' \
      -not -path './apps/control-plane/.next/*' \
      | sed 's|^./||' | sort
  fi
}

fail() {
  echo "FAIL  $*"
  FAILED=$((FAILED + 1))
}

# --- 1. hadolint -------------------------------------------------------------
HADOLINT="${HADOLINT_BIN:-$(command -v hadolint || true)}"
if [[ -z "$HADOLINT" && -x /home/user/tools/hadolint ]]; then
  HADOLINT=/home/user/tools/hadolint
fi
if [[ -n "$HADOLINT" ]]; then
  echo "== hadolint ($("$HADOLINT" --version 2>&1 | head -1))"
  while IFS= read -r dockerfile; do
    [[ "$dockerfile" == buyer-release/* ]] && continue
    if out=$("$HADOLINT" "$dockerfile" 2>&1); then
      echo "PASS  hadolint $dockerfile (no findings)"
    else
      fail "hadolint $dockerfile"
      echo "$out"
    fi
  done < <(list_files | grep -i 'dockerfile')
else
  echo "SKIP  hadolint not installed (set HADOLINT_BIN to run this section)"
fi

# --- 2. docker compose config ------------------------------------------------
COMPOSE="${COMPOSE_BIN:-$(command -v docker-compose || true)}"
if [[ -z "$COMPOSE" && -x /home/user/tools/docker-compose ]]; then
  COMPOSE=/home/user/tools/docker-compose
fi
if [[ -n "$COMPOSE" ]]; then
  echo "== docker compose ($("$COMPOSE" version 2>&1 | head -1))"
  # CI-only placeholder values: the files are fail-closed and refuse to render
  # without them, which is itself the property under test.
  ENV_FILE="$(mktemp)"
  trap 'rm -f "$ENV_FILE"' EXIT
  cat >"$ENV_FILE" <<'EOF'
POSTGRES_PASSWORD=ci-not-a-real-password
POSTGRES_SYSTEM_PASSWORD=ci-not-a-real-password
POSTGRES_APP_PASSWORD=ci-not-a-real-password
POSTGRES_USER=sniper
POSTGRES_DB=sniper
SNIPER_IMAGE=sniper-suite:local
CONTROL_PLANE_IMAGE=sniper-control-plane:local
SNIPER_PUBLIC_HOST=sniper.example.test
SNIPER_UPSTREAM=sniper:8080
NEXT_PUBLIC_API_ORIGIN=http://localhost:8080
SOLANA_KEYPAIR_HOST_PATH=/nonexistent/ci-placeholder.json
BACKUP_AGE_RECIPIENT=age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq
BACKUP_OFFSITE_TARGET=s3://ci-placeholder/sniper
TRUSTED_PROXIES=127.0.0.1/32
SNIPER_METRICS_ALLOW_CIDR=127.0.0.1/32
WEBHOOK_SECRET_ENCRYPTION_KEY=0000000000000000000000000000000000000000000000000000000000000000
MFA_ENCRYPTION_KEY=0000000000000000000000000000000000000000000000000000000000000000
API_BIND_HOST=127.0.0.1
API_PORT=8080
CONTROL_PLANE_BIND_HOST=127.0.0.1
CONTROL_PLANE_PORT=3000
DEMO_API_PORT=8080
DEMO_CONTROL_PLANE_PORT=3000
RUST_LOG=info
AWS_ACCESS_KEY_ID=ci-placeholder
AWS_SECRET_ACCESS_KEY=ci-placeholder
AWS_DEFAULT_REGION=us-east-1
AWS_ENDPOINT_URL=http://127.0.0.1:9000
AWS_SERVER_SIDE_ENCRYPTION=AES256
BACKUP_BASE_INTERVAL_HOURS=24
BACKUP_BASE_RETAIN=7
BACKUP_INTERVAL_HOURS=6
BACKUP_OFFSITE_INCLUDE_PITR=false
BACKUP_OFFSITE_MAX_FILES=100
BACKUP_RETENTION_DAYS=30
BACKUP_WAL_HEARTBEAT_SECS=60
EOF
  # Any ${VAR} this list does not name gets a shape-appropriate placeholder.
  while IFS= read -r var; do
    grep -q "^${var}=" "$ENV_FILE" && continue
    case "$var" in
      *_CIDR | *_PROXIES) value="127.0.0.1/32" ;;
      *_PORT) value="8080" ;;
      *_HOST) value="127.0.0.1" ;;
      *_URL | *_TARGET | *_ORIGIN) value="http://127.0.0.1:1/placeholder" ;;
      *) value="1" ;;
    esac
    echo "${var}=${value}" >>"$ENV_FILE"
  done < <(list_files | grep -E 'docker-compose.*\.ya?ml$' |
    xargs grep -oh '\${[A-Z_][A-Z0-9_]*' | sed 's/^\${//' | sort -u)

  check_compose() {
    local label="$1"
    shift
    if out=$("$COMPOSE" --env-file "$ENV_FILE" "$@" config -q 2>&1); then
      echo "PASS  compose config: $label"
    else
      fail "compose config: $label"
      echo "$out" | head -5
    fi
  }

  check_compose "docker-compose.yml" -f docker-compose.yml
  check_compose "docker-compose.test.yml" -f docker-compose.test.yml
  check_compose "deploy/demo/docker-compose.demo.yml" -f deploy/demo/docker-compose.demo.yml
  check_compose "base + backup overlay" \
    -f docker-compose.yml -f deploy/compose/docker-compose.backup.yml
  check_compose "base + backup + PITR overlays" \
    -f docker-compose.yml -f deploy/compose/docker-compose.backup.yml \
    -f deploy/compose/docker-compose.pitr.yml
  check_compose "base + TLS overlay" \
    -f docker-compose.yml -f deploy/compose/docker-compose.tls.yml

  # Fail-closed check: without the placeholder env, the base file MUST refuse.
  if "$COMPOSE" --env-file /dev/null -f docker-compose.yml config -q >/dev/null 2>&1; then
    fail "docker-compose.yml rendered without its required environment (should fail closed)"
  else
    echo "PASS  docker-compose.yml refuses to render without required env (fail-closed)"
  fi
else
  echo "SKIP  docker compose CLI not installed (set COMPOSE_BIN to run this section)"
fi

# --- 3. live registry digest verification ------------------------------------
if [[ -f scripts/verify-image-digests-live.py ]] && command -v python3 >/dev/null; then
  echo "== live registry manifest lookup"
  if out=$(python3 scripts/verify-image-digests-live.py 2>&1); then
    echo "$out" | grep -E "^(PASS|library|[0-9]+ distinct|NOTE)" | tail -12
    echo "PASS  image digests resolve in the live registry"
  else
    fail "image digest verification"
    echo "$out" | tail -12
  fi
else
  echo "SKIP  scripts/verify-image-digests-live.py or python3 unavailable"
fi

echo "----------------------------------------"
if [[ "$FAILED" -eq 0 ]]; then
  echo "container_assets_live: PASS (no image built, no container run — by design)"
  exit 0
fi
echo "container_assets_live: $FAILED FAILURE(S)"
exit 1
