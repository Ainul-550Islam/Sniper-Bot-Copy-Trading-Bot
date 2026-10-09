# Audit Round 4 — Money Paths & Secret Custody

**Date:** 2026-10-07 · **Scope:** the externally-triggered money surface and secret custody: provider webhook verification (`saas/provider.rs`), billing webhooks (`billing_webhook.rs`, `payment_webhooks.rs`), custody credential model (`core/custody/credentials.rs`), the custody control-plane (`saas/custody.rs`), wallet-access boundary (`wallet_access.rs`), and the sign boundary (`custody/sign_boundary.rs`, `sign_request.rs`).

**Verification status (honest):** static review only — no `cargo`/`rustc` in this workspace; nothing compiled or test-run. CI (`cargo fmt/clippy/build/test`) remains the sign-off gate.

---

## Findings fixed this round

### F-4-1 · P2 — `payment_webhooks.rs` claimed payment/invoice events as *fully processed* while applying nothing
The payment/invoice arms of `apply_normalized` are explicitly audit-only ("for now audit only"), yet the pipeline claimed the durable `provider_events` row and flipped it to `processed=true`. The marker's contract is "fully handled" — marking events processed that produced no durable record misleads operators and forensics (the record will say a payment event was fully processed when nothing was persisted).
**Fix:** verified payment/invoice events now take an explicit *observation-only* branch in `handle_webhook` — audited (`saas.billing.webhook.observed`, with an `audit-only` note) and acknowledged with `"status": "observed_audit_only"`, without claiming or completing a dedup row. When durable payment persistence lands, that branch is replaced and the claim path applies.

### F-4-2 · P2 — uncapped provider event ids in `payment_webhooks.rs`
`billing_webhook.rs` caps envelope ids at 200 chars (memory-bounding, tested); `payment_webhooks.rs` did not — a hostile/buggy signer of the webhook secret could store unbounded ids in `provider_events`.
**Fix:** `verify_request` now trims, rejects blank, and caps `id` at 200 chars; blank `type` is also rejected (parity with the billing envelope rules).

### F-4-3 · P2 — dishonest `md5` module in `payment_webhooks.rs`
A local `mod md5` was actually truncated SHA-256, plus a dead `HexWrap` type with an unused `LowerHex` impl — misleading naming at a money boundary.
**Fix:** module deleted; the dedup `payload_hash` now uses real `hex::encode(Sha256::digest(…))` (parity with `billing_webhook.rs`), documented as a drift detector, never a trust anchor.

### F-4-4 · P1 — `panic!` in custody credential API (`with_metadata`)
`CredentialRef::with_metadata` **panicked** on secret-looking metadata — a library boundary reachable from any caller violates the zero-panic objective.
**Fix:** now returns `Result<Self, String>`; single caller (the module's own test) updated; new tests pin both the refusal and every PEM private-key header now rejected (`RSA/EC/DSA/OPENSSH/ENCRYPTED` added to the marker list in `validate_reference`).

### F-4-5 · P1 — production `unwrap()` in the custody `resolve_signer` handler
`verdict.deny_reason().unwrap()` on the refusal path of `GET /api/saas/custody/signers/:id/resolve` — a panic-prone call in production code.
**Fix:** replaced with `.map(|r| r.as_str()).unwrap_or("denied")`; the debug payload still identifies the verdict.

### F-4-6 · P2 — poisoned-mutex panics on webhook marker locks
`memory_markers().lock().expect("marker mutex")` (4 sites, `billing_webhook.rs`) and `memory_seen_set().lock().expect("mutex")` (3 sites, `payment_webhooks.rs`) would take the process down on lock poisoning — the same class fixed in PumpPortal in Round 2.
**Fix:** all 7 sites now use the poison-tolerant `unwrap_or_else(|p| p.into_inner())` pattern (a poisoned marker map errs toward re-processing, which the durable claim layer then arbitrates — never toward a panic).

### F-4-7 · P3 — dishonest comment in `resolve_signer`
The comment claimed the check was only "signer active and tenant not closed", while the code pins a full policy check to `module.sniper`.
**Fix:** comment rewritten to state the pin honestly and why: the endpoint is a GET with no module parameter and answers the narrow "resolvable for the primary trading module" question; the engine's actual module is enforced where it matters — the sign boundary builds its own `CustodyRequest`. Warns future editors not to widen the pin without adding a module parameter.

---

## Verified clean this round (no changes needed)

| Area | Files | Verdict |
|---|---|---|
| Webhook signature scheme | `crates/server/src/saas/provider.rs` | HMAC-SHA256 over `"{timestamp}.{body}"`, ±300 s tolerance, constant-time compare, empty secret refuses to configure, `Debug` redacted, verification failures never echo material, envelope ids capped. |
| Billing webhook pipeline | `crates/server/src/saas/billing_webhook.rs` | Single trust decision via adapter; atomic claim (`INSERT … ON CONFLICT DO NOTHING RETURNING`); in-flight claim ⇒ 503 "retry later" (not a false duplicate); completed claim ⇒ 200 duplicate; rejection releases the claim so corrected retries work; closed tenants never mutated; client-supplied status fields never trusted; unknown types ignored-but-marked; unconfigured provider ⇒ 501 even with a perfect signature. |
| Payment webhook verification | `crates/server/src/saas/payment_webhooks.rs` | Same HMAC scheme + tolerance; durable claim errors fail closed (no process-local downgrade after a DB failure); memory fallback check-and-claim under one lock. |
| Credential references | `crates/core/src/custody/credentials.rs` | References validated secret-free (PEM markers, long-hex heuristic, env-identifier carve-out), `Debug` shows only keys, store rejects duplicates. |
| Wallet access boundary | `crates/server/src/saas/wallet_access.rs` | Public-data-only bindings (no field for key material), cross-tenant refused before anything else with existence-indistinguishable responses, `decide()` = ownership ∧ active ∧ module ∧ `wallet.manage` ∧ entitlement, thorough cross-tenant e2e-style tests. |
| Custody control plane | `crates/server/src/saas/custody.rs` | Profile/signer registry carries public metadata only; signer views tested secret-free; capability attach requires `wallet.manage` + tenant scoping + durable write checked row-count; audit recorded on every mutation. |
| Sign boundary | `crates/server/src/custody/sign_boundary.rs`, `sign_request.rs` | Exemplary: Guard 0 remote opt-in (`LIVE_CUSTODY`) → Guard 1 policy (tenant lifecycle/ownership/capability/module/provider pin) → Guard 2 health+resolution (no local fallback; Vault signer can't be satisfied by a local wallet) → Guard 3 provider signing; every outcome incl. refusal audited; local provider *refuses honestly* instead of fabricating signatures; requests are validated value objects (64-hex digest, bounded purpose) with no override flag and no key material possible. |

---

## Change table (Round 4)

| File | Change |
|---|---|
| `crates/server/src/saas/payment_webhooks.rs` | observation-only payment/invoice branch; event-id cap + blank-field rejection; real SHA-256 payload hash; `md5`/`HexWrap` deleted; poison-tolerant locks |
| `crates/server/src/saas/billing_webhook.rs` | 4 × poison-tolerant marker locks |
| `crates/core/src/custody/credentials.rs` | `with_metadata` → `Result` (no panic); full PEM private-key marker list; 2 new tests |
| `crates/server/src/saas/custody.rs` | zero-panic refusal reason in `resolve_signer`; honest module-pin documentation |

## Sign-off checklist (open, unchanged)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo build --workspace` / `cargo test --workspace -- --test-threads=1` (needs `POSTGRES_URL`, `POSTGRES_MIGRATION_URL`, `REDIS_URL`)
- [ ] `npm ci && npm run typecheck/test/test:e2e` in `apps/control-plane`
- [ ] `./scripts/verify-migration-graph.sh`, `./scripts/export-openapi.sh --check`
