# `deploy/` — production deployment configuration

Everything here is **code**, gated in CI by the `deploy-config` job. A TLS
misconfiguration, a staging/production credential overlap, or a mutable
image tag fails the build the same way a clippy warning does.

```
deploy/
├── nginx/
│   ├── sniper-suite.conf        TLS reverse proxy (envsubst template)
│   └── tls-params.conf          shared TLS parameters
├── compose/
│   └── docker-compose.tls.yml   nginx + certbot overlay
├── environments/
│   ├── staging.env.template     staging delta over .env.template
│   └── production.env.template  production delta over .env.template
└── release/
    ├── base-images.lock.json    every base image, pinned by digest
    └── README.md                the deployment ledger and rollback model
```

## The three problems this directory solves

| Problem before | Fix |
|---|---|
| The control plane bound to loopback and the only TLS guidance was a code comment. There was no committed, verifiable edge configuration. | `deploy/nginx/` + `scripts/verify-tls-config.sh` |
| Staging and production were the same `.env.template` with different values typed by hand. Nothing stopped staging from holding a production database URL or a `sk_live_` key. | `deploy/environments/` + `scripts/verify-environment-separation.sh` |
| Every image was referenced by **tag**. `postgres:16-alpine` is a moving target, so "redeploy the same version" was not reproducible and a rollback did not necessarily roll anything back. | digest pins everywhere + `scripts/verify-image-digests.sh` + a deployment ledger |

## Commands

```bash
# ---- gates (offline, seconds, run in CI) --------------------------------
./scripts/verify-tls-config.sh                  # static edge config
./scripts/verify-environment-separation.sh      # templates
./scripts/verify-image-digests.sh               # no mutable tag anywhere
./scripts/pin-base-image-digests.sh --check     # is the lock still current?

# ---- on the deploy host -------------------------------------------------
./scripts/verify-environment-separation.sh --filled   # real values
SNIPER_PUBLIC_HOST=app.example.com ./scripts/verify-tls-config.sh --live

# ---- first-time TLS bootstrap ------------------------------------------
SNIPER_PUBLIC_HOST=app.example.com SNIPER_ACME_EMAIL=ops@example.com \
  ./scripts/bootstrap-tls.sh

# ---- deploy / rollback --------------------------------------------------
./scripts/deploy-release.sh staging
./scripts/deploy-release.sh production --image ghcr.io/acme/sniper@sha256:…
./scripts/rollback-release.sh production --list
./scripts/rollback-release.sh production
```

## Order of operations on a fresh host

1. `cp deploy/environments/production.env.template deploy/environments/production.env` and fill it in.
2. `./scripts/verify-environment-separation.sh --filled`
3. `./scripts/bootstrap-tls.sh` (generates dhparam, obtains the certificate)
4. `./scripts/deploy-release.sh production --image <registry digest>`
5. `SNIPER_PUBLIC_HOST=… ./scripts/verify-tls-config.sh --live`

## Multi-replica note

Running more than one replica is safe **only** once migration
`0036_durable_tenant_controls_and_rotations.sql` is applied and the
application is the 0036-or-later build. Before it, the tenant kill-switch,
custody rotation state and WebSocket replay set were all process-local: a
second replica silently ignored tenant pauses, 404'd rotations created
elsewhere, and admitted replayed WebSocket tickets.
`crates/server/tests/multi_replica_durable_state.rs` is the regression
suite for that; run it against a real Postgres before scaling out.

## What is deliberately NOT here

* **Kubernetes manifests / Helm charts.** Compose is the supported
  topology today. Shipping an untested chart would be worse than shipping
  none: it would read as a supported path.
* **Filled-in `.env` files and `dhparam.pem`.** Generated per deployment
  and gitignored. A committed dhparam is weaker than a generated one, and
  a committed credential is a release blocker.
* **`deployments.jsonl`.** Host state, not source. It is created by
  `deploy-release.sh` on first use.
