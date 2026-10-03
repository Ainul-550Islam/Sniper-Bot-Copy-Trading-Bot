# Digest-pinned deploy and rollback

**Status:** implemented — `deploy/release/`, `scripts/deploy-release.sh`, `scripts/rollback-release.sh`
**Gate:** `./scripts/verify-image-digests.sh` (CI job `deploy-config`)

## Why tags are banned

`postgres:16-alpine` today and `postgres:16-alpine` next month are
different images. A tag is a mutable pointer; a digest is the image.
The consequences of pinning by tag:

* "redeploy the same version" is not reproducible,
* a rollback does not necessarily roll anything back,
* the build you verified in staging is not provably the build that
  reached production.

So **every** `FROM` and every compose `image:` in this repository uses
`repo@sha256:…`, and `scripts/verify-image-digests.sh` fails the build if
even one does not.

The one permitted exception is a variable reference
(`image: ${SNIPER_IMAGE:-sniper-suite:local}` on the `bot` service): the
deploy pipeline substitutes a resolved digest, and the default is only
for `docker compose up --build` during development.

## The lock file

`deploy/release/base-images.lock.json` records what each base image tag
resolved to, and when. Regenerate with:

```bash
./scripts/pin-base-image-digests.sh
```

It queries the registry HTTP API directly (no Docker daemon), reads the
digest from the `Docker-Content-Digest` **response header** — the
authoritative value, rather than re-deriving the registry's canonical
manifest form locally — and records the multi-platform index digest
where one exists, so the same pin is correct on amd64 and arm64.

`--check` compares the lock to live registry state. CI runs it as
**advisory**: an upstream rebuild (security patches) is news, not a
broken pull request. See `deploy/release/README.md` for the review flow.

## Deploy

```bash
./scripts/deploy-release.sh staging
./scripts/deploy-release.sh production --image ghcr.io/acme/sniper@sha256:…
```

Six steps, in order, each of which can abort the deploy:

1. **Preflight.** Image digest gate, `--filled` environment separation,
   TLS configuration. Production additionally refuses a dirty git tree —
   a digest that cannot be mapped back to a commit makes the ledger
   entry unverifiable.
2. **Resolve.** Whatever the operator passed becomes `repo@sha256:…`. A
   tag is resolved first; the digest is what gets recorded and rolled
   out. Production refuses a local-only image that was never pushed:
   you cannot roll back to something no registry can serve you.
3. **Migrate.** `--migrate-only`, once, from the image being deployed,
   before any new replica starts. A failure here rolls out nothing — the
   old image keeps serving.
4. **Roll.** Start the new image.
5. **Verify.** `/api/health`, then a check that the **running container
   is actually the requested image**. "Deploy succeeded but nothing
   changed" is a real failure mode and this step is what catches it.
   Production also runs the live TLS probe.
6. **Record.** Append to `deploy/release/deployments.jsonl`.

On a failed verification the script **rolls back automatically** and
exits non-zero. It never leaves an environment on an unverified image.
If there is no previous verified image to return to, it says so loudly
rather than pretending the rollback happened.

## Rollback

```bash
./scripts/rollback-release.sh production --list   # what can I roll back to?
./scripts/rollback-release.sh production          # previous verified image
./scripts/rollback-release.sh production --to ghcr.io/acme/sniper@sha256:…
```

The script will only roll into an image the ledger records as `verified`
**in that environment**. Rolling into an image that never passed
verification there is not a rollback; it is an unplanned deploy during an
incident.

It prints a plan, requires the operator to type the environment name,
pulls the target (and fails if it is no longer retrievable), rolls,
health-checks, and appends to the ledger.

## Rollback is not a database rollback

Migrations are forward-only and additive, so an older binary runs against
a newer schema. Safe through 0036 with **one exception the script prints
before acting**:

> An application build predating **0036** reads module controls, custody
> rotation state and WebSocket replay state from process memory. While
> rolled back: a tenant pause in `tenant_module_controls` is not
> honoured, rotations in `custody_rotations` are invisible to the API,
> and WebSocket replay protection is no longer shared across replicas.
> **Scale to one replica before rolling back past 0036.**

## Current pins

| Image | Pinned in |
|---|---|
| `rust:1.98.1-bookworm` | `Dockerfile` (builder) |
| `debian:bookworm-slim` | `Dockerfile` (runtime) |
| `postgres:16-alpine` | `docker-compose.yml` |
| `redis:7-alpine` | `docker-compose.yml` |
| `nginx:1.27-alpine` | `deploy/compose/docker-compose.tls.yml`, `scripts/verify-tls-config.sh` |
| `certbot/certbot:v3.1.0` | `deploy/compose/docker-compose.tls.yml` |

Digests live in `deploy/release/base-images.lock.json`.

## Known limitations

* **The application image is not signed.** Digest pinning proves
  immutability, not provenance. Cosign/Sigstore signing and an
  admission-time verification step are a reasonable next increment and
  are **not** implemented — do not claim supply-chain attestation for
  the runtime image on the strength of this document. (An SBOM is
  produced separately: `scripts/generate-sbom.sh`,
  `sbom.cyclonedx.json`.)
* **Compose only.** There is no Kubernetes rollout/rollback integration.
* **The ledger is host-local.** Back it up with the host. Without it a
  rollback is guesswork.
