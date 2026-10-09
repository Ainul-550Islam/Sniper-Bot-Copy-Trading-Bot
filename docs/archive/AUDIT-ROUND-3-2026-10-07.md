# Audit Round 3 — Credential & Session Security Core

**Date:** 2026-10-07 · **Scope:** everything *underneath* the auth surfaces hardened in Rounds 1–2: session machinery, authorization engine, SaaS auth flows, API keys, WebSocket auth/replay, MFA/TOTP stack, audit trail, invitation lifecycle.

**Verification status (honest):** all conclusions are static review. No `cargo`/`rustc` exists in this workspace; nothing in this round was compiled or test-run. CI (`cargo fmt/clippy/build/test`) remains the required sign-off gate. The one fix delivered this round follows the existing module conventions (`Decision` vocabulary, fail-closed policy, hash-only secrets) and contains no `unwrap`/`panic!` in production paths.

---

## P1 — Found and FIXED this round

### F-3-1 · Tenant IP allowlist was stored and reported but never enforced
**Files:** `crates/server/src/saas/security.rs` (write path), `crates/server/src/saas/middleware.rs`, `crates/server/src/saas/auth_flows.rs`

`POST /api/saas/security/ip-allowlist` persists CIDRs into
`tenant_security_policies.ip_allowlist` and `GET .../security/status` reports
them back — but a workspace-wide grep showed **zero readers** outside those
two handlers. A tenant that configured an allowlist believed its control
plane was origin-restricted while every origin was still admitted. Silent
security no-op.

**Fix delivered:**
- **NEW** `crates/server/src/saas/ip_allowlist.rs` — dependency-free CIDR
  parsing/matching (v4+v6, family-mismatch never matches, `/0`..full-prefix
  mask math), client-address extraction (`x-forwarded-for` leftmost entry →
  `x-real-ip` fallback), and `enforce()` with fail-closed semantics:
  - allowlist configured + no determinable client address → **DENY**;
  - policy row unloadable/undecodable → **DENY 503** (a store hiccup must
    not switch the control off);
  - no policy row or empty list → allow (control is opt-in).
- `crates/server/src/saas/mod.rs` — module registered.
- `crates/server/src/saas/middleware.rs` — `enforce()` wired into **all
  three credential branches** (tenant API key, user session incl.
  enrollment-only sessions, legacy deployment key), each immediately after
  the tenant's `can_authenticate` status check, so the rule applies
  uniformly regardless of credential type.
- `crates/server/src/saas/auth_flows.rs` — `accept_invite` (public route,
  acts ON the invited tenant) now takes `HeaderMap` and runs the same check
  before consuming the per-invitation rate budget; refusal goes through the
  standard `deny_response` (audited).
- Unit tests for CIDR parsing, mask math (v4/v6, `/0`, `/32`, family
  mismatch) and header extraction precedence.

**Deployment note (documented in the module):** header-derived addresses
are trustworthy only behind an edge that overwrites `x-forwarded-for` or
sets `x-real-ip` (standard trusted-proxy arrangement).

---

## Verified clean this round (no changes needed)

| Area | Files | Verdict |
|---|---|---|
| Session model & validation | `crates/core/src/session/{mod,model}.rs` | Hash-only storage (no token field exists on the record), pure `validate()`, revocation wins over expiry, unscoped sessions cannot serve tenant requests, plaintext never serialisable (pinned by test). |
| Session persistence | `crates/server/src/saas/store.rs` | Sessions persist as full serde JSON round-trips in `saas_runtime_records`; `mfa_policy_updated_at` / `mfa_enrollment_only` survive the round-trip (`#[serde(default)]` covers legacy rows). Read failures are returned, never reinterpreted as "no sessions". |
| RBAC + deployment limiter | `crates/core/src/auth.rs` | SHA-256-only key handling, bounded bucket map that fails closed at cap (`max_buckets` sweep then refuse), zero-rpm disables cleanly. |
| Authorization engine | `crates/core/src/authorization/{mod,context,decision}.rs` | Fixed gate order (auth → ownership → tenant state → permission → entitlement); ownership before role; 403-not-404 (no existence leak); scopes only narrow; suspended membership ⇒ empty permission set; API keys never confer platform scope; platform crossing requires role **and** user flag; boundary tests pin that ALLOW here cannot bypass TASK 5 risk or TASK 6 lease fencing. |
| Middleware | `crates/server/src/saas/middleware.rs` | Enrollment-only sessions rejected by ordinary handlers and immutably tenant-bound; MFA-currency re-check (`mfa_reauthentication_required` only re-routes to enrollment endpoints); missing tenant context is always DENY; caller-supplied org id never widens access. |
| Login/register/profile | `crates/server/src/saas/users.rs` | Dummy-hash compare for unknown accounts, length floors before PBKDF2, MFA-protected orgs must be selected at login (no unscoped bypass), password change + revoke-all-sessions in ONE transaction, logout revokes only via the caller's own session list. |
| Invitation flow | `crates/server/src/saas/auth_flows.rs`, `team.rs` | Token is bearer proof, hash-only storage, `FOR UPDATE` locking, single-use with conflict detection, resend **rotates** the hash (old token dies), 7-day expiry, MFA-enrollment-only session for enforced tenants. |
| Tenant API keys | `crates/server/src/saas/api_keys.rs` | Secret shown once, hash-only storage, creator-permission ceiling (`contains_all`, not size compare), platform scope forbidden, revocation tenant-scoped by construction, plan limit checked before creation with storage failures ≠ zero usage. |
| WebSocket auth | `crates/server/src/saas/websocket_auth.rs`, `websocket_replay_store.rs` | Query-string credentials rejected; replay claim is a single atomic `INSERT … ON CONFLICT … RETURNING` (no read-then-write); expiry is a predicate not a sweep dependency; store outage ⇒ refuse (fail-closed); hash-only storage; local fallback keeps poisoned-mutex fail-closed. |
| MFA/TOTP stack | `crates/server/src/saas/security.rs` | AES-256-GCM secrets with CSPRNG nonces; RFC-6238 test vector pinned; monotonic DB counter (`counter BIGINT NOT NULL DEFAULT 0` + `WHERE counter < $n`) prevents code replay incl. skew windows; constant-time compare; per-device+org rate limits; enforcement requires a verified device first; enrollment readiness probes key material + CSPRNG before consuming invites; credential rotation revokes sessions **and** API keys in one transaction with projection-count verification. |
| Audit trail | `crates/core/src/audit.rs` | SHA-256 hash chain, append-only (no update/delete path), `verify_chain` tamper evidence, never blocks the audited op, degradation is loud (`bot_audit_persist_failed_total` + warn). |

---

## Accepted residuals / observations

1. **Per-process sensitive limiter** (`saas/rate_limit.rs`) — documented as a per-replica control that must be paired with edge limits; no change needed, doc is honest.
2. **Invite probing is not rate-limited** for unknown tokens — acceptable: invitation tokens are 256-bit CSPRNG values hashed with SHA-256; probing is cryptographically hopeless, and valid-pending invites ARE rate-limited per invitation id.
3. **`correlation_label` is `principal@tenant`, not per-request** — documented honestly in `middleware.rs`; migration 0036 correlation columns get an actor-level label. Functional gap, not a vulnerability.
4. **Allowlist refusal string is distinguishable** from a membership refusal (403 with an allowlist-specific reason) — tells a prober the org exists and has an allowlist. Kept deliberately: tenants need the diagnostic; matches standard allowlist products.

---

## Change table (Round 3)

| File | Change |
|---|---|
| `crates/server/src/saas/ip_allowlist.rs` | **NEW** — CIDR parse/match, client-IP extraction, fail-closed `enforce()` + unit tests |
| `crates/server/src/saas/mod.rs` | register `ip_allowlist` module |
| `crates/server/src/saas/middleware.rs` | 3 × `enforce()` call sites (API key / session / legacy branches) |
| `crates/server/src/saas/auth_flows.rs` | `HeaderMap` extractor + `enforce()` before invitation consumption |

## Sign-off checklist (still open, unchanged from Round 2)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo build --workspace` / `cargo test --workspace -- --test-threads=1` (needs `POSTGRES_URL`, `POSTGRES_MIGRATION_URL`, `REDIS_URL`)
- [ ] `npm ci && npm run typecheck/test/test:e2e` in `apps/control-plane`
- [ ] `./scripts/verify-migration-graph.sh`, `./scripts/export-openapi.sh --check`
