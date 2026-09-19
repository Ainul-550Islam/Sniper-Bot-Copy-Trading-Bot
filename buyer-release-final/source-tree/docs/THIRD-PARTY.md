# Third-party & license inventory

Provenance and licensing posture of everything sniper-suite 0.1.0 depends on.
The authoritative dependency record is the two committed lockfiles — this
document does not attempt (and does not need) to enumerate every transitive
crate by hand; the tooling below does that reproducibly.

## 1. Authoritative sources

| Artifact | Scope | Size |
|---|---|---|
| `Cargo.lock` | Application workspace (7 crates) — full resolved graph | 706 packages |
| `programs/staking-suite/Cargo.lock` | Staking program — independent lockfile (own workspace root, MSRV-aware resolution for the agave platform-tools compiler) | 580 packages |
| `deny.toml` | cargo-deny policy: advisories, bans, licenses, sources | — |
| `rust-toolchain.toml` | Pinned Rust 1.98.1 for both host projects | — |

Both lockfiles are committed, and `scripts/release-check.sh` +
`.github/workflows/ci.yml` (Security job) hard-gate them on every pass.

## 2. Major direct dependencies (application workspace)

Grouped by role; versions are pinned via `[workspace.dependencies]` in the
root `Cargo.toml` and resolved in `Cargo.lock`.

| Role | Crates | Notes |
|---|---|---|
| Async runtime / utils | `tokio`, `futures`, `futures-util`, `async-trait`, `once_cell` | |
| HTTP server / middleware | `axum`, `tower`, `tower-http` | Control plane |
| HTTP / WS clients | `reqwest`, `tokio-tungstenite`, `url` | RPC, Bot API, CLOB/Gamma, PumpPortal/Geyser WS |
| Serialization | `serde`, `serde_json`, `toml`, `bincode`, `bs58`, `hex`, `base64` | |
| Solana (host side) | `solana-sdk`, `solana-client`, `solana-program`, `solana-system-interface`, `solana-transaction-status`, `solana-account-decoder`, `spl-token`, `spl-associated-token-account` | Apache-2.0 (Anza / Solana Labs / SPL) |
| Persistence | `sqlx` (PostgreSQL), `redis` | Durable truth / coordination |
| Crypto / signing | `ed25519-dalek` (Solana keys), `k256` + `tiny-keccak` (secp256k1/keccak for EIP-712), `hmac`, `sha2` | RustCrypto ecosystem |
| Observability | `tracing`, `tracing-subscriber`, `chrono`, `uuid` | |
| Errors / misc | `anyhow`, `thiserror`, `dotenvy`, `rand`, `num-bigint` | |

Staking program (on-chain binary) direct deps are deliberately minimal:
`solana-program` 2.1, `spl-token` 6 (no-entrypoint), `spl-associated-token-account` 4
(no-entrypoint), `borsh` 1.5, `thiserror` 1, `solana-system-interface` 1.0;
dev-only (never compiled into the BPF object): `bincode`, `solana-sdk`,
`solana-client`.

The freeze pass removed every direct dependency without code references
(`tokio-util`, `sha3`, `serde_with`); they remain in `Cargo.lock` only where
still required transitively (CHANGELOG, "Fixed (engineering-freeze pass)").

## 3. Protocol / service dependencies (not libraries)

These are external services the software talks to; they are **not** bundled
and **not** owned by this project (details: `docs/IP-COMPONENTS.md` §"Summary"):

- Solana clusters (mainnet/devnet RPC + WS) — buyer-contracted providers.
- pump.fun / PumpSwap / Raydium / Jupiter on-chain programs — public
  deployed programs; addresses are configuration facts.
- PumpPortal WebSocket API and a Yellowstone-compatible Geyser provider —
  optional buyer-contracted feeds (poll fallback exists).
- Polymarket Gamma + CLOB APIs and Polygon-hosted CTF Exchange contracts —
  buyer must satisfy Polymarket's terms and any applicable law.
- Telegram Bot API — buyer supplies the bot token.

## 4. License posture

- **This project:** MIT (`LICENSE`). Copyright holder is a documented
  placeholder pending transfer (`docs/HANDOVER.md` §5.1).
- **Dependency policy (enforced, not aspirational):** `deny.toml`
  `[licenses]` allows only: MIT, MIT-0, Apache-2.0, Apache-2.0 WITH
  LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, CC0-1.0,
  CDLA-Permissive-2.0, Zlib, MPL-2.0, BSL-1.0, Unicode-3.0, Unlicense
  (confidence threshold 0.8). Anything else fails the release.
- **Copyleft note:** MPL-2.0 is file-level copyleft and is on the allow-list;
  if a buyer's policy forbids it, `cargo deny check licenses` output identifies
  the affected crates. No GPL/AGPL crates are permitted by policy.
- `[bans] multiple-versions = "warn"` (duplicate versions surface but do not
  fail); `[sources]` denies unknown registries and **all** git dependencies —
  every crate comes from crates.io.
- **Last known result:** `cargo deny check` (advisories, bans, licenses,
  sources) passed clean on both projects in the final freeze gate
  (cargo-deny 0.18.9).

## 5. Advisory scanning

- `cargo audit` runs against **both** lockfiles (app + staking program) as a
  release-check gate and a CI hard gate. Last known result: **0 findings**
  (cargo-audit 0.22.2, RustSec DB snapshot at freeze time).
- The RustSec database is a point-in-time snapshot; advisories published
  after the freeze are not reflected. Buyers must re-run periodically.

## 6. SBOM status

- **NOT EXECUTED in the build environment** (`cargo cyclonedx` /
  `cargo spdx` not installed). The command to generate a CycloneDX SBOM is
  documented in `docs/RELEASE.md`.
- Both committed `Cargo.lock` files *are* the authoritative, complete,
  machine-readable dependency record (name + version + checksum for every
  crate); an SBOM generator merely reformats them.

## 7. How a buyer reproduces all of the above

```bash
# Toolchain (rustup honors rust-toolchain.toml → 1.98.1):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env

# Advisory + policy tooling (versions used in the final gate):
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.18.9 --locked
# (or download the prebuilt musl binaries from the rustsec / cargo-deny
#  GitHub releases, as the freeze environment did)

# App workspace:
cargo audit            # 0 findings at freeze
cargo deny check       # advisories / bans / licenses / sources ok

# Staking program (separate lockfile):
cd programs/staking-suite
cargo audit
cargo deny check --config ../../deny.toml   # or copy deny.toml alongside

# Optional SBOM:
cargo install cargo-cyclonedx --locked
cargo cyclonedx --workspace --all > sbom.cyclonedx.json
```

`scripts/release-check.sh` runs the audit/deny gates for both projects
automatically as part of the 20-gate release validation.
