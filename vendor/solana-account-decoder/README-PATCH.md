# Vendored patch — solana-account-decoder{,-client-types} 2.3.13 (zstd 0.13.3 → 0.14.3)

## What this is

An **unmodified copy** of `solana-account-decoder` 2.3.13 from crates.io, plus exactly one change in
`Cargo.toml`. The sibling directory `vendor/solana-account-decoder-client-types` carries the
identical one-line change, because that crate also requires `zstd ^0.13.3` (it decodes `base64+zstd`
account data).

```diff
 [dependencies.zstd]
-version = "0.13.3"
+version = "0.14.3"
```

No Rust source in this directory was touched.

## Why

Three RustSec advisories in the shipped dependency graph are all the same crate pair:

| Advisory | Crate | Fixed in |
|---|---|---|
| RUSTSEC-2026-0391 | `zstd 0.13.3` | `zstd >= 0.14.3` |
| RUSTSEC-2026-0398 | `zstd-safe 7.3.0` | `zstd-safe >= 8.2.0` |
| RUSTSEC-2026-0403 | `zstd-safe 7.3.0` | `zstd-safe >= 8.2.0` |

They reach this workspace only through `solana-account-decoder` (pulled in by
`solana-transaction-status`, which supplies the JSON-RPC `Ui*` types). The crate uses zstd in a single
place — `UiAccount::encode` for the `Base64Zstd` encoding — via
`Encoder::new(Vec::new(), 0)` / `write_all` / `finish`, an API unchanged in zstd 0.14.

Upstream facts that rule out a plain version bump:

* `solana-account-decoder` **3.1.14** (newest 3.x) still requires `zstd ^0.13.3`;
* only agave **4.4.0** requires `zstd ^0.14.0`, and agave 4.x splits the SDK into
  `solana-transaction 5` / `solana-message 5` / `solana-pubkey 4`, i.e. a type migration across the
  whole tree (319 `solana_sdk::` references in 5 crates), not a dependency bump.

Pinning the one dependency is the smallest change that removes the advisories without changing a
single type in the API surface that the rest of the workspace compiles against.

## How it is wired

Root `Cargo.toml`:

```toml
[patch.crates-io]
solana-account-decoder = { path = "vendor/solana-account-decoder" }
solana-account-decoder-client-types = { path = "vendor/solana-account-decoder-client-types" }
```

## Verification performed

```bash
cargo build -p module-sniper -p module-copy     # the two crates that use the Ui* types
cargo test  -p solana-kit --lib --tests         # 478 lib + 12 integration binaries
cargo test  -p module-sniper --lib              # 208
cargo test  -p module-copy --lib                # 65
cargo deny check advisories                     # advisories ok (0 open)
```

## When to delete this directory

On the agave 4.x SDK migration: delete `vendor/solana-account-decoder`, drop the `[patch.crates-io]`
block, and bump `solana-transaction-status` / `solana-account-decoder` to 4.4.x.

## Licence

Apache-2.0, © Anza Maintainers — unchanged from upstream (`Cargo.toml` `license` field); the upstream
`Cargo.toml.orig` is kept alongside for diffing.
