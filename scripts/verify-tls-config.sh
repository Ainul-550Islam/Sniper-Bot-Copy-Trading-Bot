#!/usr/bin/env bash
# ============================================================================
# scripts/verify-tls-config.sh — gate the TLS edge configuration (P0 §5).
#
# Two modes:
#
#   (default)  STATIC — assert the committed nginx configuration contains
#              the controls it is supposed to, and that `nginx -t` accepts
#              it after rendering. Runs in CI; needs Docker, no network.
#
#   --live     LIVE — additionally probe a running endpoint: protocol
#              versions, redirect, HSTS, security headers, and that
#              /metrics is not public. Needs SNIPER_PUBLIC_HOST.
#
# Exit non-zero on the FIRST failure. A TLS edge that "mostly" passes is
# not a passing TLS edge.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CONF="deploy/nginx/sniper-suite.conf"
PARAMS="deploy/nginx/tls-params.conf"
LIVE=0
[[ "${1:-}" == "--live" ]] && LIVE=1

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "  ok   $*"; }

require() {
    local pattern="$1" file="$2" what="$3"
    grep -Eq -- "$pattern" "$file" || fail "$what (expected /$pattern/ in $file)"
    pass "$what"
}

refuse() {
    local pattern="$1" file="$2" what="$3"
    grep -Eq -- "$pattern" "$file" && fail "$what (forbidden /$pattern/ found in $file)"
    pass "$what"
}

echo "== static: files exist =="
for f in "$CONF" "$PARAMS" deploy/compose/docker-compose.tls.yml; do
    [[ -f "$f" ]] || fail "missing $f"
    pass "$f"
done

echo
echo "== static: protocol floor =="
require 'ssl_protocols +TLSv1\.2 +TLSv1\.3;' "$PARAMS" "TLS 1.2 + 1.3 enabled"
refuse 'ssl_protocols[^;]*(SSLv2|SSLv3|TLSv1\.0|TLSv1\.1|TLSv1 )' "$PARAMS" "no deprecated protocol enabled"

echo
echo "== static: cipher posture =="
require 'ssl_ciphers +ECDHE' "$PARAMS" "ECDHE key exchange (forward secrecy)"
refuse 'ssl_ciphers[^;]*(NULL|RC4|3DES|DES-CBC|MD5|EXPORT|aNULL|eNULL)' "$PARAMS" "no broken cipher offered"
require 'ssl_session_tickets +off;' "$PARAMS" "session tickets disabled"
require 'ssl_stapling +on;' "$PARAMS" "OCSP stapling enabled"

echo
echo "== static: redirect and ACME =="
require 'listen +80;' "$CONF" ":80 listener present (for ACME + redirect)"
require 'return +301 +https://' "$CONF" "plain HTTP redirects permanently to HTTPS"
require '/\.well-known/acme-challenge/' "$CONF" "ACME challenge path served over :80"

echo
echo "== static: security headers =="
require 'Strict-Transport-Security[^;]*max-age=[0-9]{7,}' "$CONF" "HSTS with a long max-age"
require 'X-Frame-Options +"DENY" +always' "$CONF" "X-Frame-Options: DENY"
require "frame-ancestors 'none'" "$CONF" "CSP frame-ancestors 'none'"
require 'X-Content-Type-Options +"nosniff" +always' "$CONF" "X-Content-Type-Options: nosniff"
require 'Referrer-Policy' "$CONF" "Referrer-Policy set"
require 'Permissions-Policy' "$CONF" "Permissions-Policy set"
require 'server_tokens +off;' "$CONF" "nginx version not advertised"

echo
echo "== static: exposure =="
require 'location += +/metrics' "$CONF" "/metrics has its own location block"
grep -EA4 'location += +/metrics' "$CONF" | grep -Eq 'deny +all;' \
    || fail "/metrics must 'deny all' after its allow list"
pass "/metrics denied by default"
require 'limit_req_zone' "$CONF" "edge rate-limit zones defined"
require 'proxy_set_header +X-Forwarded-Proto +\$scheme;' "$CONF" "X-Forwarded-Proto forwarded"
require 'proxy_set_header +Connection +\$connection_upgrade;' "$CONF" "WebSocket upgrade wired"

echo
echo "== static: image pinning in the TLS overlay =="
if grep -Eq 'image: +[^@]*:[^@]*$' deploy/compose/docker-compose.tls.yml; then
    fail "deploy/compose/docker-compose.tls.yml pins an image by TAG; use a digest (scripts/pin-base-image-digests.sh)"
fi
pass "every image in the TLS overlay is digest-pinned"

echo
echo "== static: nginx -t on the rendered config =="
if command -v docker >/dev/null 2>&1; then
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    # Render with the SAME substitution the container entrypoint performs.
    SNIPER_PUBLIC_HOST=example.test \
    SNIPER_UPSTREAM=bot:8080 \
    SNIPER_METRICS_ALLOW_CIDR=10.0.0.0/8 \
        envsubst '${SNIPER_PUBLIC_HOST} ${SNIPER_UPSTREAM} ${SNIPER_METRICS_ALLOW_CIDR}' \
        < "$CONF" > "$tmp/sniper-suite.conf"
    cp "$PARAMS" "$tmp/tls-params.conf"

    # nginx -t opens the certificate and dhparam files, so give it
    # throwaway ones. This validates SYNTAX and directive placement, which
    # is what a config gate is for.
    mkdir -p "$tmp/certs/live/example.test" "$tmp/www"
    openssl req -x509 -nodes -newkey rsa:2048 -days 1 \
        -keyout "$tmp/certs/live/example.test/privkey.pem" \
        -out    "$tmp/certs/live/example.test/fullchain.pem" \
        -subj '/CN=example.test' 2>/dev/null
    cp "$tmp/certs/live/example.test/fullchain.pem" "$tmp/certs/live/example.test/chain.pem"
    openssl dhparam -out "$tmp/dhparam.pem" 1024 2>/dev/null

    docker run --rm \
        -v "$tmp/sniper-suite.conf:/etc/nginx/conf.d/default.conf:ro" \
        -v "$tmp/tls-params.conf:/etc/nginx/tls-params.conf:ro" \
        -v "$tmp/dhparam.pem:/etc/nginx/dhparam.pem:ro" \
        -v "$tmp/certs:/etc/letsencrypt:ro" \
        nginx@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10 \
        nginx -t
    pass "nginx accepts the rendered configuration"
else
    echo "  SKIP docker not available — 'nginx -t' not run (static assertions above still ran)"
fi

if [[ "$LIVE" -eq 0 ]]; then
    echo
    echo "STATIC TLS CONFIGURATION: PASS"
    echo "(run with --live against a deployed host to probe the real endpoint)"
    exit 0
fi

# ---------------------------------------------------------------------------
# LIVE probes
# ---------------------------------------------------------------------------
: "${SNIPER_PUBLIC_HOST:?--live needs SNIPER_PUBLIC_HOST}"
HOST="$SNIPER_PUBLIC_HOST"

echo
echo "== live: deprecated protocols refused =="
for proto in tls1 tls1_1; do
    if echo | openssl s_client -connect "$HOST:443" -"$proto" >/dev/null 2>&1; then
        fail "$HOST accepted $proto"
    fi
    pass "$proto refused"
done

echo
echo "== live: modern protocols accepted =="
for proto in tls1_2 tls1_3; do
    echo | openssl s_client -connect "$HOST:443" -"$proto" >/dev/null 2>&1 \
        || fail "$HOST refused $proto"
    pass "$proto accepted"
done

echo
echo "== live: certificate validity =="
echo | openssl s_client -connect "$HOST:443" -servername "$HOST" 2>/dev/null \
    | openssl x509 -noout -checkend 604800 \
    || fail "certificate for $HOST expires within 7 days (or could not be read)"
pass "certificate valid for at least 7 more days"

echo
echo "== live: HTTP redirects to HTTPS =="
code="$(curl -s -o /dev/null -w '%{http_code}' "http://$HOST/")"
[[ "$code" == "301" ]] || fail "http://$HOST/ returned $code, expected 301"
pass "301 redirect"

echo
echo "== live: response headers =="
headers="$(curl -sS -D - -o /dev/null "https://$HOST/api/health")"
for h in "strict-transport-security" "x-frame-options" "x-content-type-options" "referrer-policy"; do
    grep -iq "^$h:" <<<"$headers" || fail "missing response header: $h"
    pass "$h present"
done
grep -iq '^server: *nginx/[0-9]' <<<"$headers" && fail "Server header leaks the nginx version"
pass "no version in Server header"

echo
echo "== live: /metrics is not public =="
code="$(curl -s -o /dev/null -w '%{http_code}' "https://$HOST/metrics")"
[[ "$code" == "403" || "$code" == "404" ]] \
    || fail "https://$HOST/metrics returned $code — it must not be reachable from the public internet"
pass "/metrics refused ($code)"

echo
echo "LIVE TLS EDGE: PASS"
