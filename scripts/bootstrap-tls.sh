#!/usr/bin/env bash
# ============================================================================
# scripts/bootstrap-tls.sh — one-time TLS bootstrap for the nginx edge.
#
# Solves the chicken-and-egg problem: nginx will not start without a
# certificate, and certbot's HTTP-01 challenge needs a web server on :80.
# The sequence below is the only one that works without downtime on a
# fresh host:
#
#   1. generate dhparam (nginx refuses to start without the mounted file)
#   2. start nginx with a throwaway self-signed certificate so :80 serves
#      the ACME challenge path
#   3. run certbot to obtain the real certificate
#   4. replace the self-signed cert and reload
#
# Usage:
#   SNIPER_PUBLIC_HOST=app.example.com \
#   SNIPER_ACME_EMAIL=ops@example.com \
#     ./scripts/bootstrap-tls.sh
#
# Re-running is safe: every step is skipped if its output already exists.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

: "${SNIPER_PUBLIC_HOST:?set SNIPER_PUBLIC_HOST to the public FQDN, e.g. app.example.com}"
: "${SNIPER_ACME_EMAIL:?set SNIPER_ACME_EMAIL to the address Let\'s Encrypt should notify}"

COMPOSE=(docker compose -f docker-compose.yml -f deploy/compose/docker-compose.tls.yml)
DHPARAM="deploy/nginx/dhparam.pem"

# --- 1. dhparam -------------------------------------------------------------
# 4096 bits takes a few minutes on a small host. It is generated ONCE and
# is deployment-specific: a shared, well-known dhparam is strictly weaker.
if [[ -f "$DHPARAM" ]]; then
    echo "==> dhparam already present: $DHPARAM"
else
    echo "==> generating 4096-bit dhparam (this takes several minutes)"
    openssl dhparam -out "$DHPARAM" 4096
    chmod 644 "$DHPARAM"
fi

# --- 2. throwaway certificate ----------------------------------------------
# Placed in the live/ path nginx expects so the config needs no special
# bootstrap variant. Certbot overwrites all three files in step 3.
echo "==> checking for an existing certificate for ${SNIPER_PUBLIC_HOST}"
if "${COMPOSE[@]}" run --rm --entrypoint sh certbot \
       -c "test -f /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}/fullchain.pem" 2>/dev/null; then
    echo "    certificate already exists — skipping self-signed bootstrap"
else
    echo "==> installing a temporary self-signed certificate"
    "${COMPOSE[@]}" run --rm --entrypoint sh certbot -c "
        set -e
        mkdir -p /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}
        openssl req -x509 -nodes -newkey rsa:2048 -days 1 \
            -keyout /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}/privkey.pem \
            -out    /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}/fullchain.pem \
            -subj '/CN=${SNIPER_PUBLIC_HOST}'
        cp /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}/fullchain.pem \
           /etc/letsencrypt/live/${SNIPER_PUBLIC_HOST}/chain.pem
    "
fi

# --- 3. start the edge ------------------------------------------------------
echo "==> starting the stack so :80 can answer the ACME challenge"
"${COMPOSE[@]}" up -d bot nginx

echo "==> waiting for nginx to answer on :80"
for _ in $(seq 1 30); do
    if curl -fsS -o /dev/null "http://127.0.0.1/.well-known/acme-challenge/bootstrap-probe" \
       || curl -fsS -o /dev/null -w '%{http_code}' "http://127.0.0.1/" | grep -q .; then
        break
    fi
    sleep 2
done

# --- 4. obtain the real certificate ----------------------------------------
echo "==> requesting a certificate from Let's Encrypt"
"${COMPOSE[@]}" run --rm certbot certonly \
    --webroot --webroot-path=/var/www/certbot \
    --email "$SNIPER_ACME_EMAIL" \
    --agree-tos --no-eff-email \
    --non-interactive \
    --keep-until-expiring \
    -d "$SNIPER_PUBLIC_HOST"

echo "==> reloading nginx with the real certificate"
"${COMPOSE[@]}" exec nginx nginx -s reload

echo
echo "==> done. Verify the edge before announcing it:"
echo "    SNIPER_PUBLIC_HOST=$SNIPER_PUBLIC_HOST ./scripts/verify-tls-config.sh --live"
