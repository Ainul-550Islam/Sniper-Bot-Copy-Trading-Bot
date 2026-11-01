# Vendored patch — solana-account-decoder-client-types 2.3.13

Unmodified copy of the crates.io source with a single `Cargo.toml` change:

```diff
 [dependencies.zstd]
-version = "0.13.3"
+version = "0.14.3"
 optional = true
```

Rationale, wiring and removal instructions: see
`vendor/solana-account-decoder/README-PATCH.md`. This crate is the second (and last) place where the
`zstd ^0.13.3` requirement appeared; with both pinned, `cargo deny check advisories` reports
`advisories ok`.
