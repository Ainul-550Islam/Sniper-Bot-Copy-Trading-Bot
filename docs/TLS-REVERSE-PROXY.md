# TLS and the reverse proxy

**Status:** implemented — `deploy/nginx/`, `deploy/compose/docker-compose.tls.yml`
**Gate:** `./scripts/verify-tls-config.sh` (CI job `deploy-config`)

## Why the app does not terminate TLS itself

`sniper-suite` serves plain HTTP and binds `127.0.0.1:8080` by default.
That is deliberate:

* certificate renewal, OCSP stapling and protocol policy are an
  operational concern with a 90-day cadence; coupling them to the trading
  binary's release cycle is how certificates expire;
* the edge should absorb slow-loris, body floods and connection storms
  **before** they reach a process that holds positions open.

So the app stays loopback-bound and nginx is the only ingress. The TLS
overlay removes the app's published port entirely — there is no path to
:8080 from outside the compose network.

## Topology

```
internet ──443/TLS──> nginx ──http──> bot:8080 ──> postgres / redis
             │                          (no published port)
             └──80──> ACME challenge + 301 redirect
```

## Posture, and the reasoning

| Control | Setting | Why |
|---|---|---|
| Protocols | TLS 1.2 + 1.3 only | TLS 1.0/1.1 are deprecated (RFC 8996) and fail PCI-DSS. Nothing in the support matrix needs them. |
| Ciphers | ECDHE + AEAD only | Forward secrecy on every session. No CBC, no RSA key transport, no 3DES. |
| Cipher order | `ssl_prefer_server_ciphers off` | Correct for TLS1.3-first: a phone should pick ChaCha20, a server-class CPU AES-GCM. Forcing the server's order makes that worse. |
| Session tickets | **off** | An nginx ticket key is a long-lived secret that is not rotated by default; stealing it retroactively breaks forward secrecy. The shared session cache gives the same latency win without that exposure. |
| OCSP | stapled | The client does not leak the hostname it is visiting to the CA. |
| HSTS | 2 years, `includeSubDomains`, `preload` | See the warning below. |
| `/metrics` | allow-list + `deny all` | Metrics expose tenant counts, order volumes, error rates. Monitoring network only. |
| Rate limits | 20 r/s general, 2 r/s auth | A coarse edge guard. The per-tenant limiter inside the app remains the authoritative policy. |
| WebSocket | 1 h read timeout, buffering off | Tenant streams are long-lived; a shorter timeout cuts healthy sockets. |

> **HSTS warning.** `max-age=63072000; includeSubDomains; preload` is a
> commitment browsers honour for two years and cannot be withdrawn
> quickly. Do not enable it until TLS works on **every** subdomain of the
> public host. If in doubt, start at `max-age=300`, confirm, then raise.

## `X-Forwarded-For` and the real client IP

The app's rate limiter and audit trail key on the address nginx reports.

* If nginx **is** the edge: leave `set_real_ip_from` commented out. A
  client that sets `X-Forwarded-For` must not get to choose its own
  identity.
* If a cloud load balancer sits in front: uncomment it and list **only**
  that balancer's CIDR.

Getting this wrong fails in one of two ways: every tenant is rate-limited
as one caller, or any caller can forge its source address.

## First-time bootstrap

Chicken-and-egg: nginx will not start without a certificate, and certbot
needs a web server on :80. `scripts/bootstrap-tls.sh` resolves it:

```bash
SNIPER_PUBLIC_HOST=app.example.com \
SNIPER_ACME_EMAIL=ops@example.com \
  ./scripts/bootstrap-tls.sh
```

1. generate `deploy/nginx/dhparam.pem` (4096-bit, a few minutes, once)
2. install a 1-day self-signed certificate at the path nginx expects
3. start nginx so :80 answers the ACME challenge
4. `certbot certonly --webroot` for the real certificate
5. reload

Re-running is safe; every step is skipped if its output exists.

## Renewal

The `certbot` service renews twice a day with jitter. **nginx only
re-reads certificates on reload**, so the nginx service runs a 6-hour
reload loop. Without that loop a renewal lands on disk and the edge keeps
serving the old certificate until it expires — a silent outage roughly 60
days after deployment.

## Verification

```bash
# static — CI
./scripts/verify-tls-config.sh

# live — against a deployed host
SNIPER_PUBLIC_HOST=app.example.com ./scripts/verify-tls-config.sh --live
```

Live mode asserts: TLS 1.0/1.1 refused, 1.2/1.3 accepted, the certificate
has ≥ 7 days left, `http://` returns 301, HSTS and the security headers
are present, the `Server` header carries no version, and `/metrics` is
not reachable from outside.

`deploy-release.sh` runs the live check automatically for production and
rolls back if it fails.

## Known limitations

* **Single host.** This is a compose topology. A multi-node edge
  (keepalived, cloud LB, anycast) is out of scope and is not pretended to
  be covered.
* **mTLS is not configured.** Client-certificate authentication for the
  operator plane would be a reasonable next hardening step; it is not
  implemented, so do not claim it.
* **`nginx -t` validates syntax, not behaviour.** The live probes are the
  behavioural check — run them.
