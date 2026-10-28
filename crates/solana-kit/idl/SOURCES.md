# Vendored Raydium IDLs — provenance

Every IDL under `crates/solana-kit/idl/` is the **official** artifact published by
Raydium in the `raydium-io/raydium-idl` repository. Nothing here is
hand-written, reconstructed from memory, or scraped from a third-party mirror.

The repository was cloned with:

```
git clone --depth 1 https://github.com/raydium-io/raydium-idl.git
```

Upstream commit pinned by this batch:

```
e7e0c96fe77bcf6a020b84a44c47a722aac8e359
```

Upstream default branch at the time of vendoring: `master`
(last push: 2026-05-18T09:59:01Z).

## Files

| Vendored file | Upstream path | Upstream commit | sha256 |
|---|---|---|---|
| `raydium_cp_swap.json` | `raydium_cpmm/raydium_cp_swap.json` | `e7e0c96fe77bcf6a020b84a44c47a722aac8e359` | `1202f6dc8e1c3216598f2ad5c620b9aa8c64ac584563fafed68125c27fb6df81` |
| `raydium_launchpad.json` | `raydium_launchpad/raydium_launchpad.json` | `e7e0c96fe77bcf6a020b84a44c47a722aac8e359` | `963189409945069409f32e345d4adf2892e0b3ca1be504372577fe34a23c6cdb` |

Raw source URLs (per file):

* <https://raw.githubusercontent.com/raydium-io/raydium-idl/e7e0c96fe77bcf6a020b84a44c47a722aac8e359/raydium_cpmm/raydium_cp_swap.json>
* <https://raw.githubusercontent.com/raydium-io/raydium-idl/e7e0c96fe77bcf6a020b84a44c47a722aac8e359/raydium_launchpad/raydium_launchpad.json>

## Program ids declared by the vendored IDLs

Both IDLs carry an `address` field. They are the single source of truth for
the program ids re-exported from `crate::consts`:

| IDL | `address` | `metadata.name` | `metadata.version` |
|---|---|---|---|
| `raydium_cp_swap.json` | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | `raydium_cp_swap` | `0.2.0` |
| `raydium_launchpad.json` | `LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj` | `raydium_launchpad` | `0.2.0` |

## Related upstream sources used for the *implementation* (not vendored)

The quote maths and the account/PDA seeds are ported from Raydium's own
sources, which are pinned here for review. They are **not** vendored into this
repository — only the IDLs above are.

| What | Repository | Commit | Path |
|---|---|---|---|
| CPMM constant-product curve + fee maths | `raydium-io/raydium-cp-swap` | `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5` | `programs/cp-swap/src/curve/{constant_product,calculator,fees}.rs` |
| CPMM PDA seeds | `raydium-io/raydium-cp-swap` | `f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5` | `programs/cp-swap/src/states/{pool,config,oracle}.rs`, `programs/cp-swap/src/lib.rs` |
| LaunchLab curve maths + PDA seeds | `raydium-io/raydium-sdk-V2` | `f8549145afe75d4c486e30c43331c2c4da32b1eb` | `src/raydium/launchpad/{curve/*,pda.ts,instrument.ts}` |

Raw URLs:

* <https://github.com/raydium-io/raydium-cp-swap/tree/f4b9b26342afe9122d5ff67cd7a4f19ded4ac0a5>
* <https://github.com/raydium-io/raydium-sdk-V2/tree/f8549145afe75d4c486e30c43331c2c4da32b1eb>

## Verification

`scripts/verify-raydium-idl.sh` recomputes the sha256 of every `idl/*.json`
and compares it with the table above. It exits non-zero on any drift.

`crates/solana-kit/tests/raydium_cpmm_discriminators.rs` (and the LaunchLab
counterpart) asserts, for **every** instruction and account used by the
adapters, that:

1. `sha256("global:<name>")[..8] == sha256("account:<Name>")[..8]` matches the
   Anchor sighash convention, and
2. the same byte string equals the `discriminator` field recorded in the
   vendored IDL JSON above.

So an IDL edit that changes a discriminator fails the test suite rather than
silently breaking a transaction.
