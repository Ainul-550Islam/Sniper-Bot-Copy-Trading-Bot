-- 0023_trading_tenant_columns.sql — STEP 1 (enterprise tenant isolation):
-- explicit `organization_id` ownership on EVERY core trading-truth table.
--
-- WHY THIS MIGRATION EXISTS
-- -------------------------
-- Migrations 0001–0016 built the trading core for a single deployment:
-- every order, execution, position, trade, fill, intent, claim, ledger
-- event and reconciliation row implicitly belonged to "the deployment".
-- The SaaS control plane (0017–0022) then introduced organizations and
-- tenant-scoped CONTROL data (memberships, subscriptions, custody
-- profiles, billing, lifecycle), but the trading TRUTH itself stayed
-- tenant-blind: no column on `orders` says which organization placed
-- it, and no query on `positions` can be constrained by tenant at the
-- database level. That is the first hard blocker of the enterprise
-- buyer gap: Tenant A must never be able to read, mutate, sign,
-- broadcast, reconcile or stream Tenant B's trading data, and that
-- guarantee has to exist in the schema, not only in HTTP middleware.
--
-- WHAT THIS MIGRATION DOES
-- ------------------------
-- Adds ONE nullable `organization_id uuid` column per trading-truth
-- table (32 tables), each with a named foreign key to
-- `organizations(id) ON DELETE RESTRICT`. Nothing else.
--
--   * Nullable BY DESIGN: existing single-deployment rows have no
--     organization yet. 0024 backfills every legacy row
--     deterministically to the deployment organization (slug
--     `deployment` — the organization `ensure_deployment_organization`
--     in crates/server/src/saas/mod.rs already provisions and
--     reuses; no second tenant is invented) and only then enforces
--     NOT NULL. No random organization is ever assigned, and rows
--     that cannot be mapped safely make 0024 fail in a controlled
--     way instead of silently becoming cross-tenant data.
--   * NULL therefore means exactly one thing between 0023 and 0024:
--     "legacy row, ownership not yet backfilled". After 0024 it must
--     never occur. Tenant-scoped repositories (db/orders_tenant.rs,
--     db/positions_tenant.rs, db/executions_tenant.rs, db/copy_tenant.rs,
--     db/polymarket_tenant.rs) never write NULL and never return rows
--     whose organization_id differs from the acting tenant.
--   * ON DELETE RESTRICT: trading and financial truth is never
--     silently cascade-deleted. Removing an organization is governed
--     by the retention workflow (tenant_lifecycle_jobs +
--     retention_policies, 0021), which purges scoped data explicitly;
--     RESTRICT makes any premature `DELETE FROM organizations` fail
--     loudly instead of orphaning or destroying the audit trail.
--   * Forward-only, additive, restart-safe: `ADD COLUMN IF NOT EXISTS`
--     for the columns and a `pg_constraint`-guarded DO block for the
--     foreign keys (the idempotency pattern 0019 introduced), so the
--     file re-applies cleanly and behaves identically on a fresh
--     database and on an existing deployment.
--   * UUID conventions: `uuid` exactly like `organizations.id` (0017)
--     and every `organization_id` column in 0017–0022.
--
-- BACKFILL EXPECTATIONS (executed by 0024, documented here)
-- ----------------------------------------------------------
--   * Every pre-0023 row in every table below belongs to the single
--     legacy deployment and is backfilled to the deployment
--     organization's id.
--   * Tables whose existing PRIMARY KEY or UNIQUE constraint is not
--     tenant-composite today become tenant-composite in 0024 so the
--     same external identifier can exist independently per tenant and
--     a valid identifier from one tenant can never be replay-collapsed
--     against another. The known set, discovered from 0002–0015:
--       - orders.idempotency_key (UNIQUE)            -> (organization_id, idempotency_key)
--       - idempotency_keys (scope, key) PK            -> (organization_id, scope, key)
--       - dedup_keys (namespace, key) PK              -> (organization_id, namespace, key)
--       - reconciliation_state (kind, subject) PK     -> (organization_id, kind, subject)
--       - execution_intents.intent_id PK              -> (organization_id, intent_id)
--       - execution_claims.execution_id PK            -> (organization_id, execution_id)
--       - execution_lifecycle.intent_id PK            -> (organization_id, intent_id)
--       - copy_leaders.address PK                     -> (organization_id, address)
--       - copy_events.event_id PK                     -> (organization_id, event_id)
--       - copy_links.position_id PK                   -> (organization_id, position_id)
--       - poly_signals.signal_id PK                   -> (organization_id, signal_id)
--       - poly_orders.venue_order_id PK               -> (organization_id, venue_order_id)
--       - poly_fills.fill_id PK                       -> (organization_id, fill_id)
--       - ledger_events.event_id PK                   -> (organization_id, event_id)
--       - global_positions.position_key PK            -> (organization_id, position_key)
--       - global_risk_decisions.decision_id PK        -> (organization_id, decision_id)
--       - kill_switches.scope PK                      -> (organization_id, scope)
--     Logical intent/claim identifiers such as `snipe:<mint>`,
--     `copy:<wallet>:<mint>` and `poly:entry:<token>` (0016 comments)
--     are only globally unique while there is exactly one tenant;
--     the tenant-composite keys above are what makes two tenants
--     sniping the same mint, following the same leader or trading the
--     same Polymarket token independent after 0024.
--   * Chain-signature PKs (transactions.signature, trades.id,
--     positions.id, orders.id, ledger_postings (event_id, seq)) stay
--     as they are: on-chain signatures and uuid-derived app ids are
--     globally unique by construction; 0024 adds tenant-leading
--     indexes for them instead.
--
-- WHAT DELIBERATELY STAYS OUT OF THIS FILE (and why)
-- ---------------------------------------------------
--   * NOT NULL, tenant-leading composite indexes and the composite
--     uniqueness above: 0024_trading_tenant_backfill_constraints.sql.
--     Enforcing them here would break existing rows that legitimately
--     have no organization yet.
--   * operators, api_keys (0001): deployment-level credentials; 0017
--     deliberately kept them and added tenant-scoped saas_api_keys.
--   * wallets, strategies, config_versions, system_events (0001):
--     deployment-level configuration and platform log. The
--     tenant-owned equivalents are the custody bindings (0020,
--     extended by 0026) and the per-tenant runtime instances with
--     their own config/risk-policy versions (0025).
--   * runtime_flags (0010): the deployment-global process control
--     plane (whole-deployment kill switch / module gates propagated
--     across replicas). Per-tenant enable/disable, lifecycle and
--     fencing live on tenant runtime instances (0025), not here.
--   * recovery_checkpoints (0005) and the ha_* tables (0016):
--     worker-scoped HA infrastructure (worker identities, singleton
--     role leases, feed cursors, feed gaps, recovery records).
--     Per-tenant runtimes isolate these through their worker/lease/
--     scope KEYS (0025 carries the worker/lease metadata); they
--     record process state, not trading truth.
--   * All 0017–0022 SaaS tables already carry organization_id.
--
-- HASH-CHAIN SAFETY (audit_events)
-- --------------------------------
-- `audit_events` is append-only and hash-chained. The chain hash
-- covers EXACTLY (prev_hash | ts | actor | action | target | outcome |
-- detail) — see `AuditRepo::chain_hash` in crates/core/src/db/repo.rs —
-- and both the INSERT and the verification read address columns BY
-- NAME (`row.try_get("...")`), so extra columns are ignored. The new
-- `organization_id` column is therefore pure chain-external metadata:
-- existing hashes stay valid, chain verification still passes, and
-- future writers must NEVER fold this column into the canonical hash
-- input (that would require a chain-format version bump and a full
-- re-walk).
--
-- Ordering: runs after 0017, so `organizations` exists and the foreign
-- keys are safe to add here. Existing primary keys, business
-- uniqueness rules, column names and all existing data are untouched.

-- ══════════════════════════════════════════════════════════════════════
-- 1. Order management system (0002)
-- ══════════════════════════════════════════════════════════════════════

-- The ONE authoritative order record. Every module (sniper, copy,
-- polymarket, contract, telegram, system) books here and
-- `idempotency_key` is the duplicate-intent boundary. Tenant ownership
-- is re-enforced on every read/write path by the tenant-scoped
-- repositories, not only by middleware.
ALTER TABLE orders ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only status transition history per order (child of orders).
ALTER TABLE order_status_history ADD COLUMN IF NOT EXISTS organization_id uuid;

-- One row per attempt/observation against an order: validate,
-- simulate, send, confirm, cancel, reconcile, recover, note.
ALTER TABLE executions ADD COLUMN IF NOT EXISTS organization_id uuid;

-- On-chain transaction bookkeeping (signer/venue attribution since
-- 0006). A transaction is tenant truth because its signer is bound to
-- exactly one tenant custody binding; the signature PK stays global
-- because an on-chain signature is unique by construction.
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Generic idempotency for non-order operations (webhook processing,
-- telegram commands, recovery actions). 0024 makes the (scope, key)
-- primary key tenant-composite so a valid key from one tenant can
-- never consume or replay-collapse another tenant's operation.
ALTER TABLE idempotency_keys ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 2. Position and trade ledger (0003)
-- ══════════════════════════════════════════════════════════════════════

-- Durable per-module positions (restart recovery loads live ones).
-- No global position lookup may survive STEP 11: every read is
-- (organization_id, ...) constrained.
ALTER TABLE positions ADD COLUMN IF NOT EXISTS organization_id uuid;

-- The per-module trade journal (fills as seen by each module).
ALTER TABLE trades ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Periodic balance truth snapshots used by the balance reconciler.
-- The `address` dimension belongs to exactly one tenant custody
-- binding; ownership must be explicit rather than derived from the
-- address string.
ALTER TABLE balance_snapshots ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 3. Deduplication, risk trail, platform audit chain (0004)
-- ══════════════════════════════════════════════════════════════════════

-- Durable dedup windows. Two tenants tracking the same leader, mint or
-- feed signature MUST dedup independently: 0024 makes the
-- (namespace, key) primary key tenant-composite, otherwise Tenant A's
-- first-arrival insert would silently swallow Tenant B's event.
ALTER TABLE dedup_keys ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Every risk decision that blocked or altered trading plus breaker
-- trips — trading truth a tenant must be able to audit for itself.
ALTER TABLE risk_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only SHA-256 hash-chained platform audit trail. The column
-- is chain-external metadata only (see HASH-CHAIN SAFETY above); it
-- exists so tenant-attributed rows can be selected and enforced
-- without walking or weakening the chain.
ALTER TABLE audit_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 4. Reconciliation work queue (0005, kind extended by 0008)
-- ══════════════════════════════════════════════════════════════════════

-- One row per entity needing external-truth verification (orders,
-- positions, transactions, polymarket orders, balances, intents).
-- Subjects can be externally-shaped ids (venue order ids, wallet
-- addresses); 0024 makes the (kind, subject) primary key
-- tenant-composite so reconciliation of two tenants never mixes.
ALTER TABLE reconciliation_state ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 5. Write-ahead intent journal (0007)
-- ══════════════════════════════════════════════════════════════════════

-- Durable pre-broadcast intent records (crash point C closure).
-- Intent ids are deterministic per (module, symbol, wallet, side) —
-- only tenant-unique today; 0024 makes the PK tenant-composite so a
-- crash-recovery scan can never attribute one tenant's ambiguous
-- broadcast to another.
ALTER TABLE execution_intents ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 6. Distributed execution ownership (0009, 0011)
-- ══════════════════════════════════════════════════════════════════════

-- One row per LOGICAL execution id with the fencing epoch. Logical
-- ids like `snipe:<mint>` and `copy:<wallet>:<mint>` are shared
-- vocabulary across tenants; 0024 makes the PK tenant-composite so
-- Tenant B can never acquire, fence or take over Tenant A's claim,
-- and claim acquisition stays atomic per (tenant, execution).
ALTER TABLE execution_claims ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only transition history for execution claims (acquired,
-- reacquired, takeover, released, handed_off, fenced,
-- renew_rejected) — the forensic ownership trail.
ALTER TABLE execution_claim_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 7. Execution lifecycle (0012)
-- ══════════════════════════════════════════════════════════════════════

-- One row per deterministic intent id covering
-- build → simulate → submit → confirm; `submitted` is written
-- write-ahead before the broadcast. PK becomes tenant-composite in
-- 0024 (same rationale as execution_intents).
ALTER TABLE execution_lifecycle ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only lifecycle transition history backing audit/forensics.
ALTER TABLE execution_lifecycle_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 8. Copy-trading engine (0013)
-- ══════════════════════════════════════════════════════════════════════

-- The tracked leaders. The SAME leader address may be followed
-- independently by multiple tenants with different statuses and
-- counters; 0024 makes the address primary key tenant-composite.
ALTER TABLE copy_leaders ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only leader lifecycle history (followed, paused, resumed,
-- unfollowed, rule changes) with the replica that made the change.
ALTER TABLE copy_leader_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- One row per processed leader-trade event (accepted or rejected) —
-- the durable "already processed" record and forensic link from a
-- leader signature to our intent and position. Event ids are
-- deterministic feed identities shared across tenants; the PK becomes
-- tenant-composite in 0024.
ALTER TABLE copy_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Follower position ↔ mirrored leader entry links, the reconciliation
-- join between observed leader activity and held positions. Links
-- must resolve inside one tenant only.
ALTER TABLE copy_links ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 9. Polymarket trading engine (0014)
-- ══════════════════════════════════════════════════════════════════════

-- Strategy decision journal (accepted AND rejected) keyed by the
-- deterministic signal id. Market identifiers are globally known;
-- tenant ownership still applies and the PK becomes tenant-composite
-- in 0024 so two tenants trading the same condition/token with equal
-- parameters never collide.
ALTER TABLE poly_signals ADD COLUMN IF NOT EXISTS organization_id uuid;

-- CLOB-level order lifecycle keyed by the venue order id (EIP-712
-- struct hash, derivable before the POST). Paper ids carry a
-- `paper:` prefix. The OMS `orders` row remains the authoritative
-- record; this table holds venue-specific detail and must be
-- tenant-owned so recovery re-adoption and reconciliation never mix
-- tenants.
ALTER TABLE poly_orders ADD COLUMN IF NOT EXISTS organization_id uuid;

-- One row per fill event keyed by venue trade id or a deterministic
-- digest — a replayed websocket event or repeated poll must never
-- double-book a fill, and never book it to the wrong tenant.
ALTER TABLE poly_fills ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only local-vs-venue reconciliation findings with the action
-- taken.
ALTER TABLE poly_recon_findings ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- 10. Global risk + accounting ledger (0015)
-- ══════════════════════════════════════════════════════════════════════

-- The ONE financial idempotency boundary across modules: every fill,
-- fee, settlement, deposit, withdrawal, transfer, funding adjustment
-- and correction becomes exactly one event keyed by a digest of
-- (kind, module, venue, wallet, reference id). Wallets are
-- tenant-owned, but ownership must be explicit and enforceable: the
-- event id PK becomes tenant-composite in 0024.
ALTER TABLE ledger_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- The balanced double-entry lines of one ledger event, per wallet.
-- Direct ownership lets postings be read and audited tenant-scoped
-- without joining through ledger_events.
ALTER TABLE ledger_postings ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Derived aggregated book snapshot per
-- (module, venue, wallet, strategy, asset, quote asset, mode). The
-- composite position_key embeds the wallet, but explicit ownership
-- (and the tenant-composite PK in 0024) is what the entitlement and
-- risk layers can enforce without parsing the key.
ALTER TABLE global_positions ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Every global risk decision (accept AND reject) with the exposure
-- snapshot it was taken against, so a verdict is reproducible later —
-- per tenant.
ALTER TABLE global_risk_decisions ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Per-venue / per-strategy kill switches engaged at runtime. A tenant
-- engaging `venue:<x>` must stop only that tenant's trading on the
-- venue; the scope PK becomes tenant-composite in 0024.
ALTER TABLE kill_switches ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only kill switch engage/release history.
ALTER TABLE kill_switch_events ADD COLUMN IF NOT EXISTS organization_id uuid;

-- Append-only accounting reconciliation findings across the four
-- record layers (OMS orders, module trades, the ledger, positions).
ALTER TABLE accounting_recon_findings ADD COLUMN IF NOT EXISTS organization_id uuid;

-- ══════════════════════════════════════════════════════════════════════
-- Foreign keys: organizations(id) ON DELETE RESTRICT (idempotent)
-- ══════════════════════════════════════════════════════════════════════
-- Same guarded pattern 0019 introduced (check pg_constraint by name),
-- applied to every table above. Constraint names are deterministic
-- (`<table>_organization_id_fkey`) so 0024 and later tooling can
-- address them explicitly. All target tables exist before this file
-- runs (0002–0015 < 0023) and organizations exists since 0017, so
-- ordering permits the FKs here. NULL organization_id (legacy rows
-- pending the 0024 backfill) satisfies the FK by SQL semantics.

DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'orders',
        'order_status_history',
        'executions',
        'transactions',
        'idempotency_keys',
        'positions',
        'trades',
        'balance_snapshots',
        'dedup_keys',
        'risk_events',
        'audit_events',
        'reconciliation_state',
        'execution_intents',
        'execution_claims',
        'execution_claim_events',
        'execution_lifecycle',
        'execution_lifecycle_events',
        'copy_leaders',
        'copy_leader_events',
        'copy_events',
        'copy_links',
        'poly_signals',
        'poly_orders',
        'poly_fills',
        'poly_recon_findings',
        'ledger_events',
        'ledger_postings',
        'global_positions',
        'global_risk_decisions',
        'kill_switches',
        'kill_switch_events',
        'accounting_recon_findings'
    ]
    LOOP
        IF NOT EXISTS (
            SELECT 1
            FROM pg_constraint c
            JOIN pg_class cl ON cl.oid = c.conrelid
            WHERE c.conname = t || '_organization_id_fkey'
              AND cl.relname = t
        ) THEN
            EXECUTE format(
                'ALTER TABLE %I ADD CONSTRAINT %I '
                'FOREIGN KEY (organization_id) REFERENCES organizations(id) '
                'ON DELETE RESTRICT',
                t,
                t || '_organization_id_fkey'
            );
        END IF;
    END LOOP;
END$$;

-- Post-apply verification (informational; run manually to audit the
-- result of this migration):
--   SELECT table_name
--   FROM   information_schema.columns
--   WHERE  column_name = 'organization_id'
--   AND    table_schema = 'public'
--   ORDER  BY table_name;                    -- must list exactly 32 tables
--   SELECT conrelid::regclass AS table_name, conname
--   FROM   pg_constraint
--   WHERE  conname LIKE '%\_organization\_id\_fkey'
--   ORDER  BY 1;                             -- must list exactly 32 FKs
