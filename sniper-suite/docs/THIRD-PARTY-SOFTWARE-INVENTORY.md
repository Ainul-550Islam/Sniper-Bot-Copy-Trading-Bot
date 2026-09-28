# Third-Party Software Inventory — sniper-suite 0.1.0

> Source data only: `Cargo.lock` (app + `programs/staking-suite/Cargo.lock`), `apps/control-plane/package-lock.json`, `sbom.json`, `licenses.json` (generated 2026-09-24). Unknown stays unknown.

## Generation

```bash
bash scripts/generate-sbom.sh        # → sbom.json (34758 B, 200 components, sha fd837e42…), sbom.cyclonedx.json
bash scripts/generate-license-report.sh # → licenses.json (107K, 707 entries, sha c1c051…), licenses.csv
cat licenses.json | jq '.[] | select(.license=="UNKNOWN") | .name' # unknown check
```

## App Rust Dependencies (direct, from Cargo.toml workspace)

| Crate | Version | License | Purpose | Direct/Transitive |
|---|---|---|---|---|
| `tokio` | 1.53 | MIT/Apache-2.0 | async runtime full | Direct |
| `axum` | 0.7 | MIT | REST + WS | Direct |
| `tower` / `tower-http` | 0.5 / 0.6 | MIT | middleware, CORS, trace | Direct |
| `serde` / `serde_json` | 1 | MIT/Apache-2.0 | serialization | Direct |
| `sqlx` | 0.8 | MIT/Apache-2.0 | Postgres, migrate | Direct |
| `redis` | 0.27 | MIT/Apache-2.0 | Redis comp, leases | Direct |
| `solana-sdk` / `solana-client` | 2.1 | Apache-2.0 | Solana types, RPC | Direct |
| `spl-token` / `spl-associated-token-account` | 6 / 4 | Apache-2.0 | SPL | Direct |
| `k256` / `ed25519-dalek` | 0.13 / 2 | MIT/Apache-2.0 / BSD-3 | crypto | Direct |
| `sha2` / `hmac` / `hex` | 0.10 / 0.12 / 0.4 | MIT/Apache-2.0 | crypto, webhook HMAC | Direct |
| `uuid` | 1 | MIT/Apache-2.0 | tenant ids, v4 serde | Direct |
| `chrono` | 0.4 | MIT/Apache-2.0 | time, serde | Direct |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | MIT | observability json | Direct |
| `bot-core` (internal) | 0.1.0 | MIT | shared types | Direct (path) |
| `solana-kit` (internal) | 0.1.0 | MIT | RPC kit | Direct (path) |
| ... | ... | ... | ... | See `licenses.json` 707 entries for full transitive |

**Staking program** (`programs/staking-suite`): `solana-program 2.1`, `borsh 1.5`, `spl-*` — see `programs/staking-suite/Cargo.lock`, built with `cargo build-sbf` agave 2.1.21.

## Frontend (apps/control-plane)

| Package | Version | License | Purpose |
|---|---|---|---|
| `next` | 16.3.6 | MIT | App Router (`CVE-2025-66478` remediation 2026-09-27; Batch-11 F-2 pin) |
| `postcss` | 8.5.23 | MIT | CSS pipeline (resolved via `next 16.3.6`; fixes the 8.4.31 chain) |
| `react` | 19 | MIT | UI |
| `typescript` | 5.x | Apache-2.0 | typecheck strict |
| `tailwindcss` | 3.x | MIT | styles |
| ... | ... | ... | `package-lock.json` 6171 lines v3, `npm ci` deterministic |

Full list: `apps/control-plane/package.json` dependencies + `licenses.json` frontend section if `license-checker` run.

## SBOM Artifacts (per-artifact sha256+size+timestamp independently)

- `sbom.json` 34758 B, sha `fd837e4260f8d0fedb9574feaf7d71c7b26a5d475eff4208424a455f3c8bd055`, CycloneDX 1.5, generated 2026-09-24T02:34:27Z
- `sbom.cyclonedx.json` same
- `licenses.json` 108621 B, sha `c1c051ca3f0436043a5b6433b120461301c0eb1d2baee19a41c01220e3b31a59`, 707 entries
- `buyer-release/sbom/sbom.json` + `buyer-release/licenses/licenses.json` same, checksums in `buyer-release/checksums/SHA256SUMS`

## Notes

- **Direct vs transitive:** `licenses.json` marks via `repository` field; `sbom.json` `purl` `pkg:cargo/...` vs `pkg:npm/...`.
- **Unknown:** Stays unknown (e.g., some crates report `UNKNOWN` — see `licenses.json` grep).
- **No vendored code:** No `vendor/` directory; all via Cargo/NPM.
- **Generated files:** `target/` excluded from package (see `scripts/verify-delivery.sh` hygiene fix).

> **Verification:** `cat sbom.json | jq .components | grep name`, `cat licenses.json | jq length` (707), `bash scripts/generate-sbom.sh`, `bash scripts/verify-buyer-package.sh`.
