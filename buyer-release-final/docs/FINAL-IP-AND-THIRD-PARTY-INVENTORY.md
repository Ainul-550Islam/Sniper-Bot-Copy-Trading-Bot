# Final IP & third-party inventory (factual, technical — not legal advice)

Prepared for the buyer-handover pass, 2026-09-19, from the final tree
(175-file baseline of the hardening pass + handover-pass documents). This is
a factual technical inventory: what is project-owned, what is third-party,
what external interfaces are assumed. **No legal conclusions are drawn** —
license compatibility review and IP transfer paperwork require the buyer's
counsel. Companion documents: `docs/IP-COMPONENTS.md` (per-component
detail), `docs/THIRD-PARTY.md` (service dependencies), `LICENSE` (MIT).

## 1. Project license / ownership

| Item | Fact |
|---|---|
| License file | `LICENSE` — MIT, "Copyright (c) 2026 sniper-suite authors" |
| Copyright holder | Generic placeholder "sniper-suite authors" — the concrete legal entity is a **documented buyer/seller fill-in** (`docs/HANDOVER.md` §5); never fabricated |
| Project-owned source | All Rust in `crates/` (7 crates, 44,088 lines of src + tests) and `programs/staking-suite/` (5 src + 1 e2e test), all SQL migrations (11), all shell tooling (3 scripts), Dockerfile/compose/CI, and all documentation — written for this project |
| Vendored/copied third-party source inside the repo | **None.** No vendored crates, no copied protocol source files. External protocols are accessed via published crates or hand-written wire code (below) |
| Generated code | None committed (borsh/serde derives expand at compile time; no codegen output in the tree) |

## 2. Third-party Rust dependencies (pinned, lockfile-recorded)

| Workspace | Lockfile | Packages in resolve graph | Lockfile SHA-256 |
|---|---|---|---|
| Application (`Cargo.lock`) | committed | **706** | `9740cac2a1fe01fd3dd181efd68098f968078871a6674235b7f0d4790ba4ad2f` |
| Staking program (`programs/staking-suite/Cargo.lock`) | committed | **580** | `2a00d2817f5538125459a25b3fa43b504b16bfb774d42f2cc8dbc9824af6a9be` |

Machine-readable trees: `evidence/sbom/cargo-tree-app.txt` (894 unique
entries) and `cargo-tree-staking.txt` (714) in the release package. No
dependency changed during the hardening or handover passes (lockfile hashes
stable across both).

Principal direct dependencies (from the workspace `Cargo.toml` files):

* **Solana:** `solana-sdk`, `solana-client` (app RPC/keys/tx), and for the
  on-chain program `solana-program 2.1`, `spl-token 6`,
  `spl-associated-token-account 4`, `solana-system-interface 1.0`,
  `borsh 1.5`, `thiserror 1` (all from crates.io; pinned in the lockfile).
* **HTTP/WS/async:** `tokio`, `axum`, `tower`, `tower-http`, `reqwest`,
  `futures`, `tokio-tungstenite`.
* **Data/crypto primitives:** `serde`/`serde_json`, `sqlx` (Postgres),
  `redis`, `sha2`, `hmac`, `k256` + `tiny-keccak` (hand-rolled EIP-712),
  `bincode`, `hex`, `base64`, `num-bigint`, `rand`, `uuid`, `chrono`,
  `ed25519-dalek` (via solana crates).
* **Observability:** `tracing`, `tracing-subscriber`, `metrics`-style
  Prometheus text rendered by hand (`crates/core/src/obs/metrics.rs` — no
  metrics framework dependency).

## 3. Hand-written protocol integrations (no vendor SDK used)

These are project-owned implementations of PUBLIC protocol interfaces —
external ABI/layout assumptions the buyer inherits:

| Integration | Implementation | External assumption |
|---|---|---|
| Polymarket CLOB/Gamma + EIP-712 v2 orders | `crates/module-polymarket/` (auth, clob, ws, eip712, orders, ctf, collateral, gamma, strategy) — built directly on reqwest/tungstenite/k256/tiny-keccak; **no Polymarket SDK dependency** | Documented contract addresses (CTFExchangeV2, NegRiskCtfExchangeV2, pUSD proxy, CTF) + 11-field Order type + selectors (70a08231/313ce567/dd62ed3e); API drift is an external risk (documented in `docs/BUYER-RISK-REGISTER.md`) |
| Telegram Bot API | `crates/module-telegram/src/api.rs` — plain HTTPS calls; **no teloxide/telegram SDK** | Bot API method shapes; token is buyer-supplied |
| pump.fun / PumpSwap / Raydium AMM / Jupiter | `crates/solana-kit/src/{pump,pumpswap,raydium,jupiter,layout,consts}.rs` — instruction bytes built by hand from documented discriminants/layouts | Program ids + account layouts verified against devnet/mainnet reads during development; `LayoutStore` learns confirmed layouts at runtime and falls back to the documented constant form |
| Metaplex token metadata | `programs/staking-suite/src/processor.rs` — `CreateMetadataAccountV3` built by hand (discriminant **33**, verified against the deployed mainnet-beta program + source tag v1.14.0); **no mpl crate dependency in the program** | mpl-token-metadata ABI (instruction discriminant + DataV2 layout); the validator e2e clones the REAL program and executes against it |
| Yellowstone-style Geyser feeds | `crates/solana-kit/src/events.rs` — `transactionSubscribe` client | Buyer-supplied Geyser endpoint; mock-tested, provider e2e not executed (documented) |

## 4. License policy enforcement (executed, not aspirational)

* `deny.toml`: SPDX allow-list of 14 permissive licenses (MIT, MIT-0,
  Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
  ISC, CC0-1.0, CDLA-Permissive-2.0, Zlib, MPL-2.0, BSL-1.0, Unicode-3.0,
  Unlicense); crates.io-only sources; all-features graph scan.
* Executed results (hardening pass, re-run at final release gate):
  `cargo deny check` → advisories ok, bans ok, **licenses ok**, sources ok;
  `cargo audit` both lockfiles → 0 errors, 9 allow-listed warnings
  (unmaintained transitive crates inside the pinned Solana 2.x legacy
  chain — enumerated with rationale in `.cargo/audit.toml`).
* MPL-2.0 is on the allow-list: if any MPL-2.0 crate appears in a future
  dependency change, its file-level copyleft obligations attach to that
  crate's own files — flagged here as a factual note for counsel, not a
  conclusion.

## 5. Non-Rust third-party components

| Component | Source | Notes |
|---|---|---|
| Rust toolchain | rust-lang (MIT/Apache-2.0) | pinned 1.98.1 (`rust-toolchain.toml`, Dockerfile, CI) |
| Agave/Solana CLI + platform-tools | anza-xyz/agave releases | v2.1.21 tarball SHA-256 `5da3359e…`; platform-tools v1.43 |
| PostgreSQL 16 (compose pin; evidence runs on 17.11) | PostgreSQL License | service dependency, not linked code |
| Redis 7 (compose pin; evidence runs on 8.0.2) | RSALv2/SSPLv1 (Redis 7.2+) — **network service, not linked into the binary** | factual note for counsel: the app talks to Redis over the wire via the `redis` crate (MIT/Apache) |
| Debian bookworm base image | Dockerfile `rust:1.98.1-bookworm` | multi-stage, non-root final image |
| GitHub Actions runners | `.github/workflows/ci.yml` | dtolnay/rust-toolchain, solana-foundation/install-solana actions (pinned versions) |

## 6. What the buyer receives (ownership summary, factual)

1. Full copyright in all project-owned source listed in §1 (transfer terms
   are contractual — outside this document).
2. The pinned dependency set with both lockfiles (byte-frozen; hashes above).
3. Hand-written protocol integrations (§3) with their external ABI
   assumptions documented — these carry protocol-drift risk that no vendor
   SDK would have removed either.
4. All evidence artifacts (builds, tests, benches, scans) recorded in
   `docs/EVIDENCE-INDEX.md` + the release package `evidence/` tree.
5. No trademarks, no accounts, no keys, no deployed contracts: the staking
   program is UNDEPLOYED (placeholder id), all credentials are
   buyer-supplied at deployment time.
