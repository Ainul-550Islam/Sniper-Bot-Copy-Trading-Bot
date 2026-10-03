# `deploy/release/` — immutable image references and the deployment ledger

## `base-images.lock.json`

Every base image this deployment pulls, resolved from its tag to a
registry digest, with the timestamp of resolution.

**Generated, never hand-edited:**

```bash
./scripts/pin-base-image-digests.sh          # refresh
./scripts/pin-base-image-digests.sh --check  # fail if an upstream tag moved
```

The script talks to the registry HTTP API directly — no Docker daemon —
so it runs identically in CI and on a laptop. It records the
**multi-platform index digest** where the tag publishes one, so the same
pin is correct on amd64 and arm64.

`scripts/verify-image-digests.sh` enforces the two-way relationship:
every digest used in a `Dockerfile` or compose file must be in this lock,
and the lock must not drift away from the files it governs.

### When `--check` fails

It means an upstream tag now resolves to a different image. That is
**normal and expected** — base images are rebuilt for security patches.
It is not an error; it is a notification that a review is due:

1. Read the upstream changelog for the moved image.
2. `./scripts/pin-base-image-digests.sh` to refresh the lock.
3. Update the matching `FROM` / `image:` lines.
4. `./scripts/verify-image-digests.sh`
5. Deploy to staging first.

CI runs `--check` as **advisory** (`continue-on-error: true`) for exactly
this reason: a moved upstream tag must not turn every unrelated pull
request red.

## `deployments.jsonl` (created at runtime, gitignored)

Append-only ledger written by `scripts/deploy-release.sh`. One JSON object
per line:

```json
{"ts":"2026-10-02T04:11:09Z","environment":"production","image":"ghcr.io/acme/sniper@sha256:…","previous":"ghcr.io/acme/sniper@sha256:…","revision":"a1b2c3d…","status":"verified","detail":"","operator":"deploy"}
```

| field | meaning |
|---|---|
| `image` | the immutable reference that was rolled out |
| `previous` | what it replaced — the default rollback target |
| `revision` | git SHA of the source tree (production refuses a dirty tree) |
| `status` | `verified` \| `rolled_back` \| `failed` |

**This file is the rollback model.** `rollback-release.sh` will only roll
into an image this ledger records as `verified` *in that environment*.
Rolling into an image that never passed verification there is not a
rollback — it is an unplanned deploy during an incident, which is how a
bad hour becomes a bad day.

Keep the ledger. Back it up with the host. Without it, a rollback is
guesswork.

## Rollback is not a database rollback

Migrations in `crates/core/migrations/` are forward-only and additive by
policy, so an older binary runs against a newer schema. That is safe
through 0036 with **one stated exception**, which `rollback-release.sh`
prints before it acts:

> Rolling back to an application build that predates **0036** means that
> binary reads module controls, custody rotation state and WebSocket
> replay state from process memory instead of the database. While rolled
> back: a tenant pause recorded in `tenant_module_controls` is not
> honoured, rotations in `custody_rotations` are invisible to the API, and
> WebSocket replay protection stops being shared across replicas.
> **Scale to one replica before rolling back past 0036.**
