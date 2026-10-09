# Incident Response Runbook — sniper-suite 0.1.0

> No invented automation. Every step corresponds to real code/observability in this tree.

## Severity Levels

| Level | Example | Trading | Notification |
|---|---|---|---|
| **SEV1 Critical** | Double-execution, signing bypass, DB corruption, key leak | Disable immediately | Page owner, revoke, preserve evidence |
| **SEV2 High** | Billing webhook replay, cross-tenant access attempt, lease split-brain | Case-by-case | Owner + operator |
| **SEV3 Medium** | Rate-limit bypass, CORS misconfig, retention lag | Monitor | Operator |
| **SEV4 Low** | Log verbosity, metric gap | Log | — |

## Workflow

### 1. Detect

- **Signals:** `GET /health` fails, `GET /ready` not ready, `metrics` `bot_reconciliation_unresolved` >0, `AuditTrail` denied spikes, `HealthRegistry` `degraded`, logs `fenced`/`expired`, `rate_limit` 429 flood.
- **Commands:**
  ```bash
  curl -s http://localhost:8080/health | jq
  curl -s http://localhost:8080/ready | jq
  curl -s http://localhost:8080/metrics | grep bot_
  journalctl -u sniper-suite -f | grep -E "error|fenced|denied"
  ```

### 2. Classify

- **Security (auth/custody/billing)?** → SEV1/2
- **Data (DB/ledger)?** → SEV1
- **Availability (HA/queue)?** → SEV2/3
- Check `docs/SECURITY-THREAT-MODEL.md` for threat → control mapping.

### 3. Contain

- **Disable trading if required (no invented flag):**
  ```bash
  # Via API (requires Owner)
  curl -X POST http://localhost:8080/api/mode -H "x-api-key: <OWNER_KEY>" -d '{"mode":"paper"}'
  # Or kill switch per module (see src/api.rs kill_switch)
  curl -X POST http://localhost:8080/api/risk/kill-switch -H "x-api-key: <OWNER_KEY>" -d '{"scope":"global"}'
  # Or stop container
  docker stop sniper-suite
  kill -TERM <pid>
  ```
- **Revoke credentials:**
  ```bash
  # API key revoke (tenant-scoped)
  curl -X DELETE http://localhost:8080/api/saas/api_keys/<hash> -H "Authorization: Bearer <SESSION>"
  # Session revocation
  curl -X POST http://localhost:8080/api/saas/users/me/logout -H "Authorization: Bearer <TOKEN>"
  # Rotate env secrets: update Vault/KMS, then `systemctl restart sniper-suite` (no plaintext in repo)
  ```

### 4. Preserve Evidence

- **Do not `rm -rf`:** Copy, don't delete.
  ```bash
  cp -r data/journal/*.jsonl /tmp/evidence-$(date +%Y%m%d)/
  pg_dump --format=custom --file=/tmp/evidence-db-$(date +%Y%m%d).dump  # via DATABASE_URL env, see backup/commands.rs
  journalctl -u sniper-suite --since "1 hour ago" > /tmp/journal.log
  find . -name "*.log" -o -name "*.dump" | head  # hygiene check already excludes these from package
  # Audit chain (if PG)
  psql $DATABASE_URL -c "SELECT * FROM audit_trail ORDER BY id DESC LIMIT 20"
  ```
- **Record:** `docs/FINAL-EVIDENCE-CROSSWALK.md` references.

### 5. Recover (see docs/OPERATIONS-RUNBOOK.md, docs/ROLLBACK-RUNBOOK.md)

- **If DB corruption:** Restore via `backup/restore_manifest.rs` (requires verified export sha match via `preflight.rs`), then `sqlx migrate run`, `cargo test --test db_integration`.
- **If config bad:** `git checkout <last-good-tag>` + `cargo build` + `curl /health` version check.
- **If frontend:** `apps/control-plane` `npm ci && npm run build`.

### 6. Validate

```bash
bash scripts/verify-delivery.sh   # PASS (hygiene ignores target/)
bash scripts/verify-buyer-package.sh
cargo check --workspace
curl -s http://localhost:8080/ready | jq  # must be ready
```

### 7. Document

- Update `docs/FINAL-KNOWN-LIMITATIONS.md` if new limitation found.
- Update `archive/AUDIT.md` with timeline.
- If legal impact, mark `LEGAL_REVIEW_REQUIRED` in `docs/IP-OWNERSHIP-REGISTER.md`.

## Operator Decision Points

- **When to disable trading?** Any SEV1, or when `bot_reconciliation_unresolved` >0 and `startup_reconcile` reports unresolved, or when `fence` errors exceed threshold.
- **When to revoke?** On any `invalid api key` spike, or key leak suspicion (check `is_secret_like` logs for redacted key hashes only).
- **When to escalate to external audit?** On any SEV1 that touches billing/custody/tenant isolation — see `docs/PENETRATION-TEST-READINESS.md`.

> **No pentest has occurred** to validate these steps against real attack; this runbook is preparedness, not proof.
