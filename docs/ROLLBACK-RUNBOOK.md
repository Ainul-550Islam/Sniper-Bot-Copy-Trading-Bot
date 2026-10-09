# Rollback Runbook — sniper-suite 0.1.0

> **Irreversible migrations/actions are flagged.** Forward-only DB means rollback is not `migrate down`.

## 1. When to Rollback

- New version fails `bash scripts/verify-delivery.sh` (version mismatch, broken links)
- `GET /health` version not expected, or `GET /ready` not ready after deploy
- `cargo check` or `npm run build` fails on new tree
- Incident requires known-good previous tag (see `CHANGELOG.md` + `release-manifest.json` version)

## 2. Artifact Selection

- **Source:** `git tag --list` or `CHANGELOG.md` headings (e.g., `0.1.0` 2026-09-24)
- **Checksums:** Every buyer artifact has independent `SHA256+size+timestamp`:
  ```bash
  cat buyer-release/checksums/SHA256SUMS
  cat buyer-release/checksums/all-files.sha256 | sort
  sha256sum buyer-release/manifests/release-manifest.json  # compare to SHA256SUMS
  ```
- **Previous release:** `git checkout 0.1.0` or `git checkout <sha>` from `archive/AUDIT.md`

## 3. Application Rollback

```bash
git checkout <previous-tag>          # e.g., 0.1.0
cargo fmt --all --check
cargo check --workspace
cargo build --release                # or docker
docker build -t sniper-suite:rollback .
docker run -p 8080:8080 --env-file .env sniper-suite:rollback
curl -s http://localhost:8080/health | jq  # verify version == <previous-tag>
```

## 4. Database Migration Compatibility

- **Migrations:** `crates/core/migrations/0001_*.sql` → `0022_checkout_url.sql` (22, contiguous)
- **Policy:** Forward-only, no down migrations (`docs/BACKUP-RESTORE.md`, `docs/RELEASE.md` §4)
- **Irreversible:** Once `0022` applied, rolling back code to `0.0.9` that expects `0021` will still see `0022` rows; code must tolerate extra migrations (additive).
- **Check compatibility:**
  ```bash
  ls crates/core/migrations/*.sql | wc -l  # 21
  psql $DATABASE_URL -c "SELECT version FROM _sqlx_migrations ORDER BY version"
  ```
- **If rollback requires DB revert:** Must restore from verified export (see §6) — not `migrate down`.
- **Export before every deploy:**
  ```bash
  pg_dump --format=custom --file=/tmp/pre-deploy-$(date +%Y%m%d-%H%M).dump
  # Record in backup/export_manifest.rs: Documented→Executed→Verified (sha match)
  ```

## 5. Frontend Rollback

```bash
cd apps/control-plane
git checkout <previous-tag> -- apps/control-plane
npm ci --ignore-scripts               # deterministic from package-lock.json 6171 lines v3
npm run typecheck                     # strict
npm run build                         # 5 routes prerendered static
npm run lint
cd ../..
```

## 6. Release Artifact Selection

- Buyer package: `buyer-release/` contains `source/`, `docs/`, `manifests/release-manifest.json` (sha `f377bc4e…`), `sbom/sbom.json`, `licenses/licenses.json`, `checksums/SHA256SUMS`
- Verify before deploy:
  ```bash
  bash scripts/build-release-package.sh  # rebuild
  bash scripts/verify-buyer-package.sh   # PASS
  sha256sum -c buyer-release/checksums/SHA256SUMS --ignore-missing
  ```

## 7. Config Rollback

- `config.toml` + `.env` (never committed) — keep previous `.env` backup: `cp .env .env.$(date +%Y%m%d)`
- `docs/ENVIRONMENT-VARIABLE-REGISTER.md` — required/optional, secret/non-secret, default
- Check `rust-toolchain.toml` 1.98.1, `deny.toml`, `docker-compose.yml`

## 8. Post-Rollback Smoke

```bash
curl -fsS http://localhost:8080/health          # 200, version == rollback
curl -fsS http://localhost:8080/ready           # 200, or degraded if PG/Redis required
curl -fsS http://localhost:8080/metrics | head
curl -fsS http://localhost:8080/api/saas/openapi.json | jq .info.version
bash scripts/verify-delivery.sh                 # PASS (target/ INFO, not FAIL after 2026-09-24 fix)
bash scripts/final-release-check.sh             # ALL PASS (hermetic, service-backed NOT_RUN)
```

## 9. Irreversible Actions (DO NOT REVERSE)

- **DB migrations** once applied (forward-only)
- **Purge** via `retention_worker.rs` (tenant data deleted after retention)
- **Revoke** API keys / sessions (hash invalidated)
- **Burn** of staking program deployment (if `programs/staking-suite` deployed to mainnet)

> **Current version:** 0.1.0 (2026-09-24) — see `VERSION`, `Cargo.toml`, `release-manifest.json` (all 0.1.0). No earlier rollback has been executed in this tree.
