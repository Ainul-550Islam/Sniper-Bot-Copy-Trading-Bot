# API versioning and compatibility policy

**Contract version:** `1.0.0` (`saas::openapi::API_VERSION`)
**Artifact:** [`openapi/openapi.json`](../openapi/openapi.json) (authoritative) · [`openapi/openapi.yaml`](../openapi/openapi.yaml) (generated)
**Gate:** `cargo test -p sniper-suite --test openapi_artifact` (CI job `app`)

---

## What was wrong before

Three things, all of which this document and its gate fix:

1. **The contract was incomplete.** `crates/server/src/api/openapi_{billing,commercial,custody,ops}.rs`
   — about 760 lines describing the billing, commercial, custody and ops
   surfaces — were declared as modules, compiled, and **never called**.
   `GET /api/saas/openapi.json` silently omitted every path they
   describe, while the SDK and the docs implied they were present.
   `saas::openapi::merge_api_fragments` now folds them in.
2. **There was no exported artifact.** The contract only existed when a
   deployment was running. A buyer doing technical due diligence, an SDK
   generator, or a partner writing an integration had nothing to read.
3. **The version was the crate version** (`0.1.0`, hard-coded). A patch
   release of the binary looked like an API change, and an actual
   breaking change looked like nothing at all.

## The contract version is not the crate version

`API_VERSION` is tracked independently, and a test asserts the two are
not equal so neither can be bumped by accident while impersonating the
other.

| Change | Version bump | Example |
|---|---|---|
| Wording, description, example, tag | **PATCH** | clarifying what 409 means |
| New path; new optional request field; new response field; new enum value; new optional query parameter | **MINOR** | adding `/api/saas/custody/rotations` listing |
| Removing or renaming a path or `operationId`; removing a response field; making an optional field required; narrowing a type; changing a status code for an existing condition; changing auth requirements | **MAJOR** | renaming `saas.listOrders` |

### The additive rule

> A conforming client written against version *X.Y.Z* must keep working
> against every later *X.\*.\**.

"Conforming" is doing real work here: a client that breaks because we
added an unexpected JSON field was never conforming. That expectation is
stated in the contract description, not left to be discovered during an
incident.

### Things that look additive and are not

* **Adding a required request field** — breaks every existing caller.
  Add it optional with a documented default, deprecate the old shape,
  remove at the next MAJOR.
* **Adding a new enum value in a RESPONSE** — safe only if the field is
  documented as open-ended. If a client switches exhaustively on it, a
  new value is a break. This contract documents response enums as
  open-ended; clients must have a default branch.
* **Tightening validation** — a request that used to succeed and now
  returns 400 is a break, even though no schema field changed.
* **Changing a status code** — `404` → `403` for the same condition
  changes control flow in every caller. (Note the deliberate exception
  already in this API: cross-tenant access answers **404, not 403**, so
  the API is not an existence oracle. That is a security property and it
  will not change.)

## How a MAJOR version would ship

Not yet exercised — there has only ever been one version. The planned
mechanism, so it is decided before it is needed under pressure:

1. New paths under a version prefix (`/api/v2/saas/...`); existing paths
   keep working unchanged.
2. Both served concurrently for a **minimum of 6 months**.
3. The old version returns a `Deprecation` and a `Sunset` header
   (RFC 8594) from the day v2 ships.
4. Removal is a separate, announced release.

Today's paths are unprefixed (`/api/saas/...`). They are **v1 by
definition** and will not be moved; a v2 would be additive alongside
them. Rewriting existing URLs to add a `/v1` prefix would itself be the
breaking change this policy exists to prevent.

## Deprecation

A deprecated operation is marked `"deprecated": true` in the contract,
keeps working for at least 6 months, and carries a `Sunset` header. It
is removed only in a MAJOR release. No operation is deprecated today.

## Regenerating the artifact

```bash
# both files, from the code
./scripts/export-openapi.sh

# or just the authoritative JSON
UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact
```

CI runs the same test **without** `UPDATE_OPENAPI`. A stale artifact
fails the build and the failure message names the paths that were added
or removed, so a reviewer sees the contract change in the diff rather
than discovering it from a customer.

`openapi.json` is authoritative; `openapi.yaml` is generated from it by
`scripts/export-openapi.sh` and must never be hand-edited.

## What the artifact tests guarantee

| Test | Guarantee |
|---|---|
| `exported_artifact_matches_the_code` | the committed file is what the running server serves |
| `artifact_declares_the_contract_version` | `info.version` is `API_VERSION`, and distinct from the crate version |
| `every_operation_id_is_present_and_unique` | SDK generators cannot silently drop an endpoint to a name collision |
| `the_surface_fragments_are_merged_in` | the billing/custody/ops surfaces cannot fall out of the contract again |
| `no_response_schema_exposes_a_credential` | no response schema declares a password, key, token or hash field |

## Known limitations

* **The contract is hand-written, not derived from the router.** A route
  added to Axum without a matching entry here will not appear in the
  document and no test will notice. Deriving the document from the
  router (utoipa or equivalent) is the correct long-term fix and is
  **not** implemented. Reviewers: a new public route needs a contract
  entry in the same pull request — `CODEOWNERS` routes `openapi/` and
  `saas/openapi.rs` to the API owners for exactly this reason.
* **Response schemas are shallow in places** (`{"type": "object"}` for a
  few custody operations). They are honest about the status code and the
  auth requirement; they do not fully describe every body.
* **No contract tests against a running server.** The document is
  verified for internal consistency, not asserted against live
  responses.
