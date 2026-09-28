# Open-Source Compliance — sniper-suite 0.1.0

> This is an inventory-based explanation, not legal approval. Buyer must obtain own counsel. Reference `sbom.json` + `licenses.json`.

## 1. Your Code License

- `LICENSE` MIT (2026 sniper-suite authors) — permissive, requires preservation of copyright + license notice in copies.
- `Cargo.toml` `license.workspace = "MIT"` inherited by 8 members.

## 2. Dependency Licenses (actual, from `licenses.json` 707 entries, `sbom.json` 200 comps)

| Category | Licenses Found | Example Crates | Obligation (summary) |
|---|---|---|---|
| **Permissive MIT/Apache/BSD** | `MIT`, `MIT OR Apache-2.0`, `Apache-2.0`, `BSD-3-Clause`, `BSD-2-Clause`, `ISC`, `Zlib` | `tokio`, `axum`, `serde`, `sqlx`, `solana-sdk`, `k256` | Keep copyright + license notice in distributions; MIT/Apache allow commercial use. No copyleft. |
| **Dual permissive** | `Unlicense OR MIT`, `CC0-1.0 OR MIT-0 OR Apache-2.0` | `aho-corasick` (`Unlicense OR MIT`), `constant_time_eq` | Choose one — typically MIT. |
| **Copyleft (weak)** | `MPL-2.0` (rare), `LGPL` (if any) — **not observed as dominant** | — | Check `licenses.json` grep for `GPL` — currently 0 `GPL-3.0` in 707 entries (run `cat licenses.json | grep -i gpl`) |
| **Unknown** | `UNKNOWN` | Some `licenses.json` entries report `UNKNOWN` (e.g., if `license` field missing in Cargo.toml) | `LEGAL_REVIEW_REQUIRED` — buyer must treat as unknown, contact author or replace |

**Current check (2026-09-24):**
```bash
cat licenses.json | jq -r '.[].license' | sort | uniq -c | sort -rn | head -n 20
# MIT, MIT OR Apache-2.0 dominate; no GPL-3.0 found in 707 entries (as of generation)
cat sbom.json | jq '.components[] | .licenses' | sort | uniq -c
```

## 3. What You Must Do (permissive compliance)

- Preserve `LICENSE` (MIT) + per-dependency notices. `licenses.json`/`licenses.csv` already aggregated for buyer.
- For MIT/Apache/BSD: include license text in distribution if you redistribute binaries (buyer-release already includes `LICENSE` + `licenses/`).
- No source disclosure required for MIT/Apache/BSD.

## 4. Copyleft / Legal Review

- If future `cargo add` introduces `GPL`/`AGPL`/`SSPL`, you would need to: disclose source of that component, or replace. Current 707-entry inventory shows **none** — but buyer must re-run `bash scripts/generate-license-report.sh` after any Cargo change and review.
- `UNKNOWN` stays unknown — do not assume permissive. Mark `LEGAL_REVIEW_REQUIRED` (see `docs/IP-OWNERSHIP-REGISTER.md`).

## 5. Staking Program (separate)

- `programs/staking-suite/Cargo.lock` pinned to `solana 2.1` generation, `borsh 1.5`, `spl` — same MIT/Apache mix. Built with `cargo build-sbf` agave 2.1.21, not `cargo check` workspace.

## 6. Frontend

- `apps/control-plane` `next 16.3.6` MIT (with `eslint-config-next 16.3.6` MIT), `react 19` MIT, `typescript` Apache-2.0 — permissive. `package-lock.json` 6171 lines v3 is real (`npm ci --ignore-scripts` → 354 packages, 2026-09-27).
  Version history: `next 15.5.4` shipped until 2026-09-27, when it was bumped to `15.5.26` to remediate the CVSS 10.0 RCE `CVE-2025-66478` (App Router / React Server Components; fixed upstream in 15.5.7+); later the same day Batch 11 (F-2) moved the tree to `next 16.3.6`, the line in which `postcss` resolves to `8.5.23` (>= the advisory floor), so `npm audit` now reports 0 vulnerabilities.

## 7. Generated Artifacts (not licensed)

- `target/`, `node_modules/`, `sbom.json`, `licenses.json` are generated, excluded from or included as evidence, not as source to license.

## 8. References

- Generated: `sbom.json` (sha `fd837e42…`), `licenses.json` (sha `c1c051…`), `buyer-release/checksums/SHA256SUMS`
- Source: `deny.toml` (`cargo deny check licenses` allowlist), `scripts/generate-license-report.sh`
- Verification: `cargo deny check licenses` (hard gate in `ci.yml` security job), `bash scripts/verify-buyer-package.sh`

> **No legal approval:** This doc does not constitute counsel. Buyer: run `cargo deny check licenses` + `cat licenses.json` review before commercial use.
