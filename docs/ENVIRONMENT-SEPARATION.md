# Staging / production separation

**Status:** implemented — `deploy/environments/`
**Gate:** `./scripts/verify-environment-separation.sh` (CI job `deploy-config`)

## The rule

Staging exists to be broken. It must therefore be **unable** to:

* reach production's database, Redis or object storage,
* sign with production's custody keys,
* move real funds on mainnet,
* charge a real card,
* authenticate a production API key.

Each of those is asserted mechanically. Separation written only in a
runbook is not separation.

## File layout

Both environment files are a **delta over `.env.template`**, so they can
be diffed line-for-line. That diff is the artifact an auditor asks for.

```bash
set -a
. .env.template                              # every variable, documented
. deploy/environments/production.env         # the environment's overrides
set +a
```

| File | Committed? |
|---|---|
| `deploy/environments/staging.env.template` | yes — placeholders only |
| `deploy/environments/production.env.template` | yes — placeholders only |
| `deploy/environments/staging.env` | **no** — gitignored, real values |
| `deploy/environments/production.env` | **no** — gitignored, real values |

## What the gate asserts

### Template mode (CI — no secrets present)

| Assertion | Rationale |
|---|---|
| staging `ALLOW_LIVE_TRADING=false`, `SOLANA_LIVE=0`, `LIVE_BILLING=0`, `LIVE_CUSTODY=0`, `DEPLOYMENT_SMOKE_LIVE=0` | the complete set of switches that reach real money, keys or cards |
| staging `EXECUTION_MODE` ∈ {paper, dry_run} | |
| staging `SOLANA_CLUSTER` ∈ {devnet, testnet} | staging never touches mainnet |
| production pre-enables **none** of the live gates | production is promoted to live by a deliberate recorded change, never by a default |
| production `DATABASE_REQUIRED=true` | production must never silently run on the in-memory store |
| production `DATABASE_AUTO_MIGRATE=false` | see below |
| the two files name **different** hosts, users, databases, Redis URLs and keypair paths | identical placeholders are how one value ends up in both files |
| both files declare the same variable set (modulo a documented allow-list) | a variable missing from one file silently inherits a default it should have overridden |
| no non-empty secret slot, no `sk_live_`, `whsec_…`, `hvs.…` or PEM private key | a committed credential is a release blocker |

The gate strips comments before scanning, because these templates
*document* the forbidden patterns ("a `sk_live_` key here is a release
blocker"). A gate that cannot tell a warning from a credential is a gate
nobody keeps.

### Filled mode (deploy host only — never CI)

```bash
./scripts/verify-environment-separation.sh --filled
```

Compares the **real** values and fails on any shared value across a list
of 20 sensitive variables (passwords, URLs, API keys, webhook secrets,
KMS key ids, keypair paths, hostnames). It also refuses if `staging.env`
so much as mentions the production host or the production database host.

Values are never printed — only variable names.

CI must not run this: it would need both environments' secrets in one
place, which is the thing being prevented.

## Why production sets `DATABASE_AUTO_MIGRATE=false`

Five replicas rolling at once would each open the database and each run
the migrator. Even with advisory locking that is a race nobody wants
against a schema holding financial state.

Instead the schema is advanced **once**, before any new replica starts,
by the very image being deployed:

```bash
# scripts/deploy-release.sh step 3
docker compose run --rm -e DATABASE_AUTO_MIGRATE=true bot --migrate-only
```

`--migrate-only` (also `MIGRATE_ONLY=1`) is implemented in
`crates/server/src/main.rs`. It forces `enabled`, `auto_migrate` and
`required` on, applies the embedded migrations, prints the applied count
and exits. It **fails loudly** when the database is not configured: in a
deploy pipeline, "nothing was configured so I did nothing, successfully"
would roll out a build expecting a schema that was never applied.

Running the migration from the deployed build is the point — a schema
advanced by some other binary is a schema nobody verified against this
one.

## Promoting production to live

Nothing in `production.env.template` is live out of the box. Going live
is a deliberate, recorded change to specific variables:

| Variable | From | To | Precondition |
|---|---|---|---|
| `EXECUTION_MODE` | `dry_run` | `live` | signed risk mandate |
| `ALLOW_LIVE_TRADING` | `false` | `true` | funded keys |
| `SOLANA_LIVE` | `0` | `1` | mainnet RPC provisioned |
| `LIVE_CUSTODY` | `0` | `1` | `SIGNING_PROVIDER` ≠ `local`, provider reachable |
| `LIVE_BILLING` | `0` | `1` | live provider credentials |

`SIGNING_PROVIDER=local` keeps a private key on the host filesystem. That
is fine for staging and **not** acceptable for a production deployment
holding customer funds.
