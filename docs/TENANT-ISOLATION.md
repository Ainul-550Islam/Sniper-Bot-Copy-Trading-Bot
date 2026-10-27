# Tenant database isolation

## Mechanism

Migrations `0060`–`0062` establish PostgreSQL row-level security for public tables
that expose `organization_id`, `org_id`, or `tenant_id`. The policy compares the
selected tenant column with `app.current_org_id()`, which reads the transaction
setting `app.org_id` and parses it as a UUID. `USING` limits visible and
mutable rows; `WITH CHECK` prevents inserting or moving a row outside the
current tenant. Policies are enabled and forced, including for table owners.
An absent or empty GUC resolves to `NULL` and therefore reveals no tenant rows.

`bot_app` is the intended non-bypass application role. `bot_system` is a
privileged `BYPASSRLS` role reserved for reviewed identity-plane, webhook,
migration, and lifecycle work. Both are created as `NOLOGIN` group roles; a
deployment administrator must create runtime principals, grant role membership,
and keep credentials outside the repository. Never grant `bot_system` to
ordinary request handlers.

`bot_core::tenant::rls_context` is the single owner of `app.org_id`. The Axum
request layer creates a task-local context; successful `authorize_request`
resolution binds exactly one authenticated organization to it. On every app
pool checkout, `Database` executes `SET ROLE bot_app` and sets the task-local
organization with `set_config(..., false)`; it resets both the GUC and role
before releasing the connection. Unbound app requests and background work
without an explicit scope receive an empty tenant context and fail closed.
`TenantScope` provides an explicit transaction-local guard for grouped
operations. The app login must be a member of `bot_app`.

`Database` owns separate app, system, and (when migrations are enabled) DDL
pools. `PostgresSaasRepo` uses the system pool for identity/control-plane
persistence; the DDL pool is used only for migrations and role grants. The
system pool executes as `bot_system`, clears the tenant context, and must never
be used for tenant trading queries. `SystemScope` is an explicit guard for
trusted cross-tenant lifecycle work.

## Exemptions

`public.rls_exempt_tables` is an auditable registry with a required non-empty
reason. Add a table only when a tenant-keyed column is demonstrably carrying
global identity-plane data and RLS would break a reviewed global operation.
Do not exempt a table to avoid writing tenant context, to accommodate a
worker, or because a row currently has a `NULL` tenant key. The policy remains
fail-closed for null tenant keys. Every new exemption requires a security
review, an explanatory reason in the registry seed migration, and coverage-test
changes.

## Adding a tenant table safely

1. Add a UUID tenant ownership column and constraints/indexes to the table.
2. Ensure every query path resolves the acting organization from authenticated
   server context, never from an untrusted request field alone.
3. Wrap tenant DB work in `TenantScope` and execute all related statements in
   that same transaction. Keep identity-plane and lifecycle work on a distinct
   `bot_system` connection and use `SystemScope` explicitly.
4. Add cross-tenant read/write tests and run `scripts/check-rls-coverage.sh`;
   the migration's catalog assertion and `rls_coverage` test must also pass.
5. Review `pg_policies`, role grants, null-tenant behavior, and production
   connection configuration before deploying the migration.

## Rollout status

The migration foundation, task-local HTTP context, separate bot_app/bot_system/DDL
pools, explicit transaction guards, and PostgreSQL coverage tests are present.
The backtest list/detail/create handlers additionally use `TenantScope`;
tests verify pool role separation, fail-closed behavior, role/GUC cleanup, and
isolation across representative trading, control-plane, scheduling, backtest,
and audit tables. SaaS/trading HTTP requests that pass through
`authorize_request` bind their tenant before handler database queries.
Some trusted background and legacy service paths still need explicit
`TenantScope` or reviewed `SystemScope` routing; without one, the app pool
returns no tenant rows. This repository therefore does **not** claim complete
RLS coverage for every background path. Keep migrations 0060–0062 gated from
live rollout until those paths have explicit scopes and production role grants
are verified.
