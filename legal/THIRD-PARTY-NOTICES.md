# THIRD-PARTY NOTICES

Sniper-Suite builds on open-source components. This file is the notice
required by those licenses (in particular Apache-2.0 §4(d) and MPL-2.0).
It was generated from `licenses.csv` (the dependency inventory shipped at
the repository root); regenerate with `scripts/generate-license-report.sh`.

The proprietary code authored by the seller is governed by `LICENSE` and
the executed sale documents; ONLY the components listed below remain under
their original open-source licenses.

Inventory date: 2026-10-07 · 707 Rust packages.

## 1. License summary

| License expression | Packages |
|--------------------|----------|
| MIT OR Apache-2.0 | 253 |
| Apache-2.0 | 174 |
| MIT | 90 |
| MIT/Apache-2.0 | 57 |
| Apache-2.0 OR MIT | 44 |
| Unicode-3.0 | 18 |
| BSD-3-Clause | 11 |
| Unlicense OR MIT | 5 |
| Apache-2.0 OR ISC OR MIT | 5 |
| Apache-2.0/MIT | 4 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| Zlib OR Apache-2.0 OR MIT | 3 |
| Zlib | 3 |
| ISC | 3 |
| CDLA-Permissive-2.0 | 3 |
| BSD-2-Clause | 2 |
| BSD-3-Clause OR MIT OR Apache-2.0 | 2 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 2 |
| Unlicense/MIT | 2 |
| Apache-2.0 OR BSL-1.0 OR MIT | 2 |
| MPL-2.0 | 2 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 |
| 0BSD OR MIT OR Apache-2.0 | 1 |
| CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | 1 |
| BSD-3-Clause AND MIT | 1 |
| BSD-3-Clause/MIT | 1 |
| Zlib OR MIT OR Apache-2.0 | 1 |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 |
| MIT OR Apache-2.0 OR BSD-1-Clause | 1 |
| Apache-2.0 / MIT | 1 |
| MIT OR Apache-2.0 OR Zlib | 1 |
| MIT AND BSD-3-Clause | 1 |
| MIT OR Zlib OR Apache-2.0 | 1 |
| Apache-2.0 AND ISC | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| UNKNOWN | 1 |
| CC0-1.0 | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |

Where an expression contains `OR` (for example `MIT OR Apache-2.0`), the
distributor elects the first-listed alternative for the purposes of this
distribution; `AND` expressions apply cumulatively. Two packages offer
`MIT OR Apache-2.0 OR LGPL-2.1-or-later`: the MIT alternative is elected,
so no LGPL (copyleft) obligation arises from this distribution.

## 2. Resolved UNKNOWN entries

| Package | Version | Recorded | Resolved to | Basis |
|---------|---------|----------|-------------|-------|
| solana-config-program-client | 0.0.2 | UNKNOWN | Apache-2.0 | Upstream repository `github.com/solana-program/config` ships the full Apache License 2.0 text in its LICENSE file (verified 2026-10-07). crates.io metadata shows "non-standard" because the crate publishes no SPDX `license` field; the repository license governs. |

## 3. MPL-2.0 components (file-level copyleft)

The following components are licensed under the Mozilla Public License 2.0.
They bundle Mozilla's set of trusted root certificates. MPL-2.0 obligations
are satisfied for this distribution because: (a) this notice identifies the
MPL-covered files, (b) the full MPL-2.0 text is reproduced in Appendix C,
and (c) the distribution includes the Source form of the entire program, so
the recipient can obtain the MPL-covered files in source form.

| Package | Version | Repository |
|---------|---------|------------|
| webpki-roots | 0.24.0 | https://github.com/rustls/webpki-roots |
| webpki-roots | 0.25.4 | https://github.com/rustls/webpki-roots |

## 4. Unicode data (Unicode-3.0 / CDLA-Permissive-2.0)

Several packages embed Unicode character data under the Unicode License v3
or CDLA-Permissive-2.0 (both permissive). The Unicode License v3 notice is
reproduced in Appendix D.

## 5. Full inventory

| Package | Version | License | Repository |
|---------|---------|---------|------------|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://github.com/oyvindln/adler2 |
| aead | 0.5.2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| aes | 0.8.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/block-ciphers |
| aes-gcm-siv | 0.11.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/AEADs |
| agave-feature-set | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| agave-reserved-account-keys | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| ahash | 0.8.12 | MIT OR Apache-2.0 | https://github.com/tkaitchuck/ahash |
| aho-corasick | 1.1.5 | Unlicense OR MIT | https://github.com/BurntSushi/aho-corasick |
| alloc-no-stdlib | 2.0.4 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib |
| alloc-stdlib | 0.2.4 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 | https://github.com/zakarumych/allocator-api2 |
| android_system_properties | 0.1.6 | MIT OR Apache-2.0 | https://github.com/nical/android_system_properties |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| arc-swap | 1.9.2 | MIT OR Apache-2.0 | https://github.com/vorner/arc-swap |
| ark-bn254 | 0.4.0 | MIT/Apache-2.0 | https://github.com/arkworks-rs/curves |
| ark-ec | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-ff | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-ff-asm | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-ff-macros | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-poly | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-serialize | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-serialize-derive | 0.4.2 | MIT/Apache-2.0 | https://github.com/arkworks-rs/algebra |
| ark-std | 0.4.0 | MIT/Apache-2.0 | https://github.com/arkworks-rs/std |
| arrayref | 0.3.9 | BSD-2-Clause | https://github.com/droundy/arrayref |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 | https://github.com/bluss/arrayvec |
| asn1-rs | 0.5.2 | MIT/Apache-2.0 | https://github.com/rusticata/asn1-rs.git |
| asn1-rs-derive | 0.4.0 | MIT/Apache-2.0 | https://github.com/rusticata/asn1-rs.git |
| asn1-rs-impl | 0.1.0 | MIT/Apache-2.0 | https://github.com/rusticata/asn1-rs.git |
| assert_matches | 1.5.0 | MIT/Apache-2.0 | https://github.com/murarth/assert_matches |
| async-channel | 1.9.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-channel |
| async-compression | 0.4.48 | MIT OR Apache-2.0 | https://github.com/Nullus157/async-compression |
| async-lock | 3.4.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/async-lock |
| async-trait | 0.1.92 | MIT OR Apache-2.0 | https://github.com/dtolnay/async-trait |
| atoi | 2.0.0 | MIT | https://github.com/pacman82/atoi-rs |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/atomic-waker |
| atty | 0.2.14 | MIT | https://github.com/softprops/atty |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | https://github.com/cuviper/autocfg |
| axum | 0.7.9 | MIT | https://github.com/tokio-rs/axum |
| axum-core | 0.4.5 | MIT | https://github.com/tokio-rs/axum |
| axum-macros | 0.4.2 | MIT | https://github.com/tokio-rs/axum |
| backon | 1.6.0 | Apache-2.0 | https://github.com/Xuanwo/backon |
| base16ct | 0.2.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/base16ct |
| base64 | 0.12.3 | MIT/Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| base64 | 0.13.1 | MIT/Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| base64 | 0.22.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| base64ct | 1.8.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| bincode | 1.3.3 | MIT | https://github.com/servo/bincode |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| blake3 | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | https://github.com/BLAKE3-team/BLAKE3 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| block-buffer | 0.9.0 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| borsh | 0.10.4 | MIT OR Apache-2.0 | https://github.com/near/borsh-rs |
| borsh | 1.8.1 | MIT OR Apache-2.0 | https://github.com/near/borsh-rs |
| borsh-derive | 0.10.4 | Apache-2.0 | https://github.com/nearprotocol/borsh |
| borsh-derive | 1.8.1 | Apache-2.0 | https://github.com/near/borsh-rs |
| borsh-derive-internal | 0.10.4 | Apache-2.0 | https://github.com/nearprotocol/borsh |
| borsh-schema-derive-internal | 0.10.4 | Apache-2.0 | https://github.com/nearprotocol/borsh |
| bot-core | 0.1.0 | MIT |  |
| brotli | 8.0.4 | BSD-3-Clause AND MIT | https://github.com/dropbox/rust-brotli |
| brotli-decompressor | 5.0.3 | BSD-3-Clause/MIT | https://github.com/dropbox/rust-brotli-decompressor |
| bs58 | 0.5.1 | MIT/Apache-2.0 | https://github.com/Nullus157/bs58-rs |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 | https://github.com/fitzgen/bumpalo |
| bv | 0.11.1 | MIT/Apache-2.0 | https://github.com/tov/bv-rs |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| byteorder | 1.5.0 | Unlicense OR MIT | https://github.com/BurntSushi/byteorder |
| bytes | 1.12.1 | MIT | https://github.com/tokio-rs/bytes |
| caps | 0.5.6 | MIT/Apache-2.0 | https://github.com/lucab/caps-rs |
| cc | 1.4.6 | MIT OR Apache-2.0 | https://github.com/rust-lang/cc-rs |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| cfg_aliases | 0.2.2 | MIT | https://github.com/katharostech/cfg_aliases |
| cfg_eval | 0.1.2 | Zlib OR MIT OR Apache-2.0 | https://github.com/danielhenrymantilla/cfg_eval.rs |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/stream-ciphers |
| chrono | 0.4.45 | MIT OR Apache-2.0 | https://github.com/chronotope/chrono |
| cipher | 0.4.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| cmov | 0.5.4 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| combine | 4.6.8 | MIT | https://github.com/Marwes/combine |
| compression-codecs | 0.4.43 | MIT OR Apache-2.0 | https://github.com/Nullus157/async-compression |
| compression-core | 0.4.33 | MIT OR Apache-2.0 | https://github.com/Nullus157/async-compression |
| concurrent-queue | 2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/concurrent-queue |
| console | 0.15.11 | MIT | https://github.com/console-rs/console |
| console_error_panic_hook | 0.1.7 | Apache-2.0/MIT | https://github.com/rustwasm/console_error_panic_hook |
| console_log | 0.2.2 | MIT/Apache-2.0 | https://github.com/iamcodemaker/console_log |
| const-oid | 0.9.6 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/const-oid |
| constant_time_eq | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 | https://github.com/cesarb/constant_time_eq |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| crc | 3.4.0 | MIT OR Apache-2.0 | https://github.com/mrhooray/crc-rs.git |
| crc-catalog | 2.5.0 | MIT OR Apache-2.0 | https://github.com/akhilles/crc-catalog.git |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 | https://github.com/srijs/rust-crc32fast |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-queue | 0.3.14 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crunchy | 0.2.4 | MIT | https://github.com/eira-fransham/crunchy |
| crypto-bigint | 0.5.5 | Apache-2.0 OR MIT | https://github.com/RustCrypto/crypto-bigint |
| crypto-common | 0.1.6 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| crypto-mac | 0.8.0 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| ctr | 0.9.2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/block-modes |
| ctutils | 0.4.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| curve25519-dalek | 3.2.0 | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek |
| curve25519-dalek | 4.1.3 | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek/tree/main/curve25519-dalek |
| curve25519-dalek-derive | 0.1.1 | MIT/Apache-2.0 | https://github.com/dalek-cryptography/curve25519-dalek |
| darling | 0.24.1 | MIT | https://github.com/TedDriggs/darling |
| darling_core | 0.24.1 | MIT | https://github.com/TedDriggs/darling |
| darling_macro | 0.24.1 | MIT | https://github.com/TedDriggs/darling |
| dashmap | 5.5.3 | MIT | https://github.com/xacrimon/dashmap |
| data-encoding | 2.11.1 | MIT | https://github.com/ia0/data-encoding |
| der | 0.7.10 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/der |
| der-parser | 8.2.0 | MIT/Apache-2.0 | https://github.com/rusticata/der-parser.git |
| deranged | 0.5.8 | MIT OR Apache-2.0 | https://github.com/jhpratt/deranged |
| derivation-path | 0.2.0 | MIT OR Apache-2.0 | https://github.com/jpopesculian/derivation-path |
| derivative | 2.2.0 | MIT/Apache-2.0 | https://github.com/mcarton/rust-derivative |
| digest | 0.10.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| digest | 0.11.3 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| digest | 0.9.0 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 | https://github.com/yaahc/displaydoc |
| dlopen2 | 0.5.0 | MIT | https://github.com/OpenByteDev/dlopen2 |
| dlopen2_derive | 0.3.0 | MIT |  |
| dotenvy | 0.15.7 | MIT | https://github.com/allan2/dotenvy |
| ecdsa | 0.16.9 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures/tree/master/ecdsa |
| ed25519 | 1.5.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures/tree/master/ed25519 |
| ed25519 | 2.2.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures/tree/master/ed25519 |
| ed25519-dalek | 1.0.1 | BSD-3-Clause | https://github.com/dalek-cryptography/ed25519-dalek |
| ed25519-dalek | 2.2.0 | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek |
| ed25519-dalek-bip32 | 0.2.0 | MIT OR Apache-2.0 | https://github.com/jpopesculian/ed25519-dalek-bip32 |
| either | 1.18.0 | MIT OR Apache-2.0 | https://github.com/rayon-rs/either |
| elliptic-curve | 0.13.8 | Apache-2.0 OR MIT | https://github.com/RustCrypto/traits/tree/master/elliptic-curve |
| encode_unicode | 1.0.0 | Apache-2.0 OR MIT | https://github.com/tormol/encode_unicode |
| env_logger | 0.9.3 | MIT OR Apache-2.0 | https://github.com/env-logger-rs/env_logger/ |
| equivalent | 1.0.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/equivalent |
| errno | 0.3.14 | MIT OR Apache-2.0 | https://github.com/lambda-fairy/rust-errno |
| etcetera | 0.8.0 | MIT OR Apache-2.0 | https://github.com/lunacookies/etcetera |
| event-listener | 2.5.3 | Apache-2.0 OR MIT | https://github.com/smol-rs/event-listener |
| event-listener | 5.4.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/event-listener |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT | https://github.com/smol-rs/event-listener-strategy |
| fastbloom | 0.17.0 | MIT OR Apache-2.0 | https://github.com/tomtomwombat/fastbloom/ |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| feature-probe | 0.1.1 | MIT/Apache-2.0 | https://github.com/tov/feature-probe-rs |
| ff | 0.13.1 | MIT/Apache-2.0 | https://github.com/zkcrypto/ff |
| fiat-crypto | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | https://github.com/mit-plv/fiat-crypto |
| find-msvc-tools | 0.1.12 | MIT OR Apache-2.0 | https://github.com/rust-lang/cc-rs |
| five8 | 0.2.1 | MIT | https://github.com/kevinheavey/five8 |
| five8_const | 0.1.4 | MIT | https://github.com/kevinheavey/five8 |
| five8_core | 0.1.2 | MIT | https://github.com/kevinheavey/five8 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 | https://github.com/rust-lang/flate2-rs |
| flume | 0.11.1 | Apache-2.0/MIT | https://github.com/zesterer/flume |
| fnv | 1.0.7 | Apache-2.0 / MIT | https://github.com/servo/rust-fnv |
| foldhash | 0.1.5 | Zlib | https://github.com/orlp/foldhash |
| foldhash | 0.2.0 | Zlib | https://github.com/orlp/foldhash |
| foreign-types | 0.3.2 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| foreign-types-shared | 0.1.1 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| futures | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-channel | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-executor | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-intrusive | 0.5.0 | MIT OR Apache-2.0 | https://github.com/Matthias247/futures-intrusive |
| futures-io | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-macro | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-task | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-timer | 3.0.4 | MIT/Apache-2.0 | https://github.com/async-rs/futures-timer |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| generic-array | 0.14.9 | MIT | https://github.com/fizyk20/generic-array.git |
| gethostname | 0.2.3 | Apache-2.0 | https://codeberg.org/flausch/gethostname.rs.git |
| getrandom | 0.1.16 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| getrandom | 0.2.17 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| getrandom | 0.3.4 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| governor | 0.6.3 | MIT | https://github.com/boinkor-net/governor.git |
| group | 0.13.0 | MIT/Apache-2.0 | https://github.com/zkcrypto/group |
| hashbrown | 0.13.2 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | 0.14.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| hashlink | 0.10.0 | MIT OR Apache-2.0 | https://github.com/kyren/hashlink |
| heck | 0.5.0 | MIT OR Apache-2.0 | https://github.com/withoutboats/heck |
| hermit-abi | 0.1.19 | MIT/Apache-2.0 | https://github.com/hermitcore/libhermit-rs |
| hermit-abi | 0.5.3 | MIT OR Apache-2.0 | https://github.com/hermit-os/hermit-rs |
| hex | 0.4.3 | MIT OR Apache-2.0 | https://github.com/KokaKiwi/rust-hex |
| histogram | 0.6.9 | MIT/Apache-2.0 | https://github.com/brayniac/histogram |
| hkdf | 0.12.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/KDFs/ |
| hmac | 0.12.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/MACs |
| hmac | 0.8.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/MACs |
| hmac-drbg | 0.3.0 | Apache-2.0 |  |
| home | 0.5.12 | MIT OR Apache-2.0 | https://github.com/rust-lang/cargo |
| http | 0.2.12 | MIT OR Apache-2.0 | https://github.com/hyperium/http |
| http | 1.5.0 | MIT OR Apache-2.0 | https://github.com/hyperium/http |
| http-body | 1.1.0 | MIT | https://github.com/hyperium/http-body |
| http-body-util | 0.1.5 | MIT | https://github.com/hyperium/http-body |
| httparse | 1.10.1 | MIT OR Apache-2.0 | https://github.com/seanmonstar/httparse |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | https://github.com/pyfisch/httpdate |
| humantime | 2.4.0 | MIT OR Apache-2.0 | https://github.com/chronotope/humantime |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hybrid-array |
| hyper | 1.11.1 | MIT | https://github.com/hyperium/hyper |
| hyper-rustls | 0.27.9 | Apache-2.0 OR ISC OR MIT | https://github.com/rustls/hyper-rustls |
| hyper-util | 0.1.20 | MIT | https://github.com/hyperium/hyper-util |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 | https://github.com/strawlab/iana-time-zone |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 | https://github.com/strawlab/iana-time-zone |
| icu_collections | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_locale_core | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_normalizer | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_properties | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_properties_data | 2.3.0 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| icu_provider | 2.3.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| ident_case | 1.0.1 | MIT/Apache-2.0 | https://github.com/TedDriggs/ident_case |
| idna | 1.1.0 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | https://github.com/hsivonen/idna_adapter |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/indexmap |
| indicatif | 0.17.11 | MIT | https://github.com/console-rs/indicatif |
| Inflector | 0.11.4 | BSD-2-Clause | https://github.com/whatisinternet/inflector |
| inout | 0.1.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| ipnet | 2.12.2 | MIT OR Apache-2.0 | https://github.com/krisprice/ipnet |
| itertools | 0.10.5 | MIT/Apache-2.0 | https://github.com/rust-itertools/itertools |
| itertools | 0.12.1 | MIT OR Apache-2.0 | https://github.com/rust-itertools/itertools |
| itertools | 0.13.0 | MIT OR Apache-2.0 | https://github.com/rust-itertools/itertools |
| itoa | 1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| jni | 0.22.4 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-rs |
| jni-macros | 0.22.4 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-rs |
| jni-sys | 0.4.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| jni-sys-macros | 0.4.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| jobserver | 0.1.35 | MIT OR Apache-2.0 | https://github.com/rust-lang/jobserver-rs |
| js-sys | 0.3.105 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys |
| jsonrpc-core | 18.0.0 | MIT | https://github.com/paritytech/jsonrpc |
| k256 | 0.13.4 | Apache-2.0 OR MIT | https://github.com/RustCrypto/elliptic-curves/tree/master/k256 |
| kaigan | 0.2.6 | MIT | https://github.com/metaplex-foundation/kaigan |
| keccak | 0.1.6 | Apache-2.0 OR MIT | https://github.com/RustCrypto/sponges/tree/master/keccak |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 | https://github.com/rust-lang-nursery/lazy-static.rs |
| libc | 0.2.189 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| libm | 0.2.16 | MIT | https://github.com/rust-lang/compiler-builtins |
| libredox | 0.1.24 | MIT | https://gitlab.redox-os.org/redox-os/libredox.git |
| libsecp256k1 | 0.6.0 | Apache-2.0 | https://github.com/paritytech/libsecp256k1 |
| libsecp256k1-core | 0.2.2 | Apache-2.0 | https://github.com/paritytech/libsecp256k1 |
| libsecp256k1-gen-ecmult | 0.2.1 | Apache-2.0 | https://github.com/paritytech/libsecp256k1 |
| libsecp256k1-gen-genmult | 0.2.1 | Apache-2.0 | https://github.com/paritytech/libsecp256k1 |
| libsqlite3-sys | 0.30.1 | MIT | https://github.com/rusqlite/rusqlite |
| litemap | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| log | 0.4.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| lru-slab | 0.1.3 | MIT OR Apache-2.0 OR Zlib | https://github.com/Ralith/lru-slab |
| matchers | 0.2.0 | MIT | https://github.com/hawkw/matchers |
| matchit | 0.7.3 | MIT AND BSD-3-Clause | https://github.com/ibraheemdev/matchit |
| md-5 | 0.10.6 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| memchr | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| memmap2 | 0.5.10 | MIT OR Apache-2.0 | https://github.com/RazrFalcon/memmap2-rs |
| memoffset | 0.9.1 | MIT | https://github.com/Gilnaa/memoffset |
| merlin | 3.0.0 | MIT | https://github.com/zkcrypto/merlin |
| mime | 0.3.17 | MIT OR Apache-2.0 | https://github.com/hyperium/mime |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 | https://github.com/Alexhuszagh/minimal-lexical |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| mio | 1.2.3 | MIT | https://github.com/tokio-rs/mio |
| module-copy | 0.1.0 | MIT |  |
| module-polymarket | 0.1.0 | MIT |  |
| module-sniper | 0.1.0 | MIT |  |
| module-telegram | 0.1.0 | MIT |  |
| nix | 0.30.1 | MIT | https://github.com/nix-rust/nix |
| no-std-compat | 0.4.1 | MIT | https://gitlab.com/jD91mZM2/no-std-compat |
| nom | 7.1.3 | MIT | https://github.com/Geal/nom |
| nonzero_ext | 0.3.0 | Apache-2.0 | https://github.com/antifuchs/nonzero_ext |
| nu-ansi-term | 0.50.3 | MIT | https://github.com/nushell/nu-ansi-term |
| num | 0.2.1 | MIT/Apache-2.0 | https://github.com/rust-num/num |
| num-bigint | 0.2.6 | MIT/Apache-2.0 | https://github.com/rust-num/num-bigint |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 | https://github.com/rust-num/num-bigint |
| num-bigint-dig | 0.8.6 | MIT/Apache-2.0 | https://github.com/dignifiedquire/num-bigint |
| num-complex | 0.2.4 | MIT/Apache-2.0 | https://github.com/rust-num/num-complex |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | https://github.com/jhpratt/num-conv |
| num-derive | 0.4.2 | MIT OR Apache-2.0 | https://github.com/rust-num/num-derive |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | https://github.com/rust-num/num-integer |
| num-iter | 0.1.46 | MIT OR Apache-2.0 | https://github.com/rust-num/num-iter |
| num-rational | 0.2.4 | MIT/Apache-2.0 | https://github.com/rust-num/num-rational |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | https://github.com/rust-num/num-traits |
| num_cpus | 1.17.0 | MIT OR Apache-2.0 | https://github.com/seanmonstar/num_cpus |
| num_enum | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://github.com/illicitonion/num_enum |
| num_enum_derive | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://github.com/illicitonion/num_enum |
| number_prefix | 0.4.0 | MIT | https://github.com/ogham/rust-number-prefix |
| oid-registry | 0.6.1 | MIT/Apache-2.0 | https://github.com/rusticata/oid-registry.git |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| opaque-debug | 0.3.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| openssl | 0.10.81 | Apache-2.0 | https://github.com/rust-openssl/rust-openssl |
| openssl-macros | 0.1.1 | MIT/Apache-2.0 |  |
| openssl-probe | 0.2.1 | MIT OR Apache-2.0 | https://github.com/rustls/openssl-probe |
| openssl-sys | 0.9.117 | MIT | https://github.com/rust-openssl/rust-openssl |
| parking | 2.2.1 | Apache-2.0 OR MIT | https://github.com/smol-rs/parking |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| paste | 1.0.15 | MIT OR Apache-2.0 | https://github.com/dtolnay/paste |
| pbkdf2 | 0.11.0 | MIT OR Apache-2.0 | https://github.com/RustCrypto/password-hashes/tree/master/pbkdf2 |
| pem | 1.1.1 | MIT | https://github.com/jcreekmore/pem-rs.git |
| pem-rfc7468 | 0.7.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/pem-rfc7468 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| percentage | 0.1.0 | MIT OR Apache-2.0 |  |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| pkcs1 | 0.7.5 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/pkcs1 |
| pkcs8 | 0.10.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/pkcs8 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/pkg-config-rs |
| plain | 0.2.3 | MIT/Apache-2.0 | https://github.com/randomites/plain |
| polyval | 0.6.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/universal-hashes |
| portable-atomic | 1.15.0 | Apache-2.0 OR MIT | https://github.com/taiki-e/portable-atomic |
| potential_utf | 0.1.6 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| powerfmt | 0.2.0 | MIT OR Apache-2.0 | https://github.com/jhpratt/powerfmt |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 | https://github.com/cryptocorrosion/cryptocorrosion |
| proc-macro-crate | 0.1.5 | Apache-2.0/MIT | https://github.com/bkchr/proc-macro-crate |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 | https://github.com/bkchr/proc-macro-crate |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| qstring | 0.7.2 | MIT | https://github.com/algesten/qstring |
| quanta | 0.12.6 | MIT | https://github.com/metrics-rs/quanta |
| quinn | 0.11.12 | MIT OR Apache-2.0 | https://github.com/quinn-rs/quinn |
| quinn-proto | 0.11.18 | MIT OR Apache-2.0 | https://github.com/quinn-rs/quinn |
| quinn-udp | 0.5.15 | MIT OR Apache-2.0 | https://github.com/quinn-rs/quinn |
| quote | 1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| rand | 0.10.2 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand | 0.7.3 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand | 0.8.8 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_chacha | 0.2.2 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_core | 0.10.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand_core |
| rand_core | 0.5.1 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_core | 0.6.4 | MIT OR Apache-2.0 | https://github.com/rust-random/rand |
| rand_hc | 0.2.0 | MIT/Apache-2.0 | https://github.com/rust-random/rand |
| rand_pcg | 0.10.2 | MIT OR Apache-2.0 | https://github.com/rust-random/rngs |
| raw-cpuid | 11.6.0 | MIT | https://github.com/gz/rust-cpuid |
| rayon | 1.12.0 | MIT OR Apache-2.0 | https://github.com/rayon-rs/rayon |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 | https://github.com/rayon-rs/rayon |
| redis | 0.27.6 | BSD-3-Clause | https://github.com/redis-rs/redis-rs |
| redox_syscall | 0.5.18 | MIT | https://gitlab.redox-os.org/redox-os/syscall |
| redox_syscall | 0.9.4 | MIT | https://gitlab.redox-os.org/redox-os/kernel |
| regex | 1.13.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| reqwest | 0.12.28 | MIT OR Apache-2.0 | https://github.com/seanmonstar/reqwest |
| reqwest-middleware | 0.4.2 | MIT OR Apache-2.0 | https://github.com/TrueLayer/reqwest-middleware |
| rfc6979 | 0.4.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures/tree/master/rfc6979 |
| ring | 0.17.14 | Apache-2.0 AND ISC | https://github.com/briansmith/ring |
| rsa | 0.9.10 | MIT OR Apache-2.0 | https://github.com/RustCrypto/RSA |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | https://github.com/rust-lang/rustc-hash |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | https://github.com/djc/rustc-version-rs |
| rusticata-macros | 4.1.0 | MIT/Apache-2.0 | https://github.com/rusticata/rusticata-macros.git |
| rustls | 0.21.12 | Apache-2.0 OR ISC OR MIT | https://github.com/rustls/rustls |
| rustls | 0.23.45 | Apache-2.0 OR ISC OR MIT | https://github.com/rustls/rustls |
| rustls-native-certs | 0.8.4 | Apache-2.0 OR ISC OR MIT | https://github.com/rustls/rustls-native-certs |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 | https://github.com/rustls/pki-types |
| rustls-platform-verifier | 0.7.0 | MIT OR Apache-2.0 | https://github.com/rustls/rustls-platform-verifier |
| rustls-platform-verifier-android | 0.1.1 | MIT OR Apache-2.0 | https://github.com/rustls/rustls-platform-verifier |
| rustls-webpki | 0.101.7 | ISC | https://github.com/rustls/webpki |
| rustls-webpki | 0.103.15 | ISC | https://github.com/rustls/webpki |
| rustversion | 1.0.23 | MIT OR Apache-2.0 | https://github.com/dtolnay/rustversion |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | https://github.com/dtolnay/ryu |
| saas-sdk | 0.1.0 | MIT |  |
| same-file | 1.0.6 | Unlicense/MIT | https://github.com/BurntSushi/same-file |
| schannel | 0.1.29 | MIT | https://github.com/steffengy/schannel-rs |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | https://github.com/bluss/scopeguard |
| sct | 0.7.1 | Apache-2.0 OR ISC OR MIT | https://github.com/rustls/sct.rs |
| sec1 | 0.7.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/sec1 |
| security-framework | 3.7.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| semver | 1.0.28 | MIT OR Apache-2.0 | https://github.com/dtolnay/semver |
| serde | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde-big-array | 0.5.1 | MIT OR Apache-2.0 | https://github.com/est31/serde-big-array |
| serde_bytes | 0.11.19 | MIT OR Apache-2.0 | https://github.com/serde-rs/bytes |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| serde_path_to_error | 0.1.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/path-to-error |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 | https://github.com/nox/serde_urlencoded |
| serde_with | 3.23.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ |
| serde_with_macros | 3.23.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ |
| sha1 | 0.10.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| sha1_smol | 1.0.1 | BSD-3-Clause | https://github.com/mitsuhiko/sha1-smol |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| sha2 | 0.9.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| sha3 | 0.10.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| sharded-slab | 0.1.7 | MIT | https://github.com/hawkw/sharded-slab |
| shlex | 2.0.1 | MIT OR Apache-2.0 | https://github.com/comex/rust-shlex |
| signal-hook | 0.3.18 | Apache-2.0/MIT | https://github.com/vorner/signal-hook |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 | https://github.com/vorner/signal-hook |
| signature | 1.6.4 | Apache-2.0 OR MIT | https://github.com/RustCrypto/traits/tree/master/signature |
| signature | 2.2.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/traits/tree/master/signature |
| simd-adler32 | 0.3.10 | MIT | https://github.com/mcountryman/simd-adler32 |
| simd_cesu8 | 1.2.0 | Apache-2.0 OR MIT | https://github.com/seancroach/simd_cesu8 |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 | https://github.com/rusticstuff/simdutf8 |
| siphasher | 0.3.11 | MIT/Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| siphasher | 1.0.3 | MIT/Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| slab | 0.4.12 | MIT | https://github.com/tokio-rs/slab |
| smallvec | 1.16.1 | MIT OR Apache-2.0 | https://github.com/servo/rust-smallvec |
| sniper-suite | 0.1.0 | MIT |  |
| socket2 | 0.5.10 | MIT OR Apache-2.0 | https://github.com/rust-lang/socket2 |
| socket2 | 0.6.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/socket2 |
| solana-account | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-account-decoder | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-account-decoder-client-types | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-account-info | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-address-lookup-table-interface | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-atomic-u64 | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-big-mod-exp | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-bincode | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-blake3-hasher | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-bn254 | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-borsh | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-client-traits | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-clock | 2.2.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-cluster-type | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-commitment-config | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-compute-budget-interface | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-config-program-client | 0.0.2 | UNKNOWN | https://github.com/solana-program/config |
| solana-connection-cache | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-cpi | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-curve25519 | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-decode-error | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-define-syscall | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-derivation-path | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-ed25519-program | 2.2.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-epoch-info | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-epoch-rewards | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-epoch-rewards-hasher | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-epoch-schedule | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-example-mocks | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-feature-gate-interface | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-feature-set | 2.2.5 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-fee-calculator | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-fee-structure | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-genesis-config | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-hard-forks | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-hash | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-inflation | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-instruction | 2.3.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-instructions-sysvar | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-keccak-hasher | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-keypair | 2.2.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-kit | 0.1.0 | MIT |  |
| solana-last-restart-slot | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-loader-v2-interface | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-loader-v3-interface | 5.0.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-loader-v4-interface | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-logger | 2.3.1 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-measure | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-message | 2.4.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-metrics | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-msg | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-native-token | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-net-utils | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-nonce | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-nonce-account | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-offchain-message | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-packet | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-perf | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-poh-config | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-precompile-error | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-precompiles | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-presigner | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-program | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-program-entrypoint | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-program-error | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-program-memory | 2.3.1 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-program-option | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-program-pack | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-pubkey | 2.4.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-pubsub-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-quic-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-quic-definitions | 2.3.1 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-rayon-threadlimit | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rent | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rent-collector | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-rent-debits | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-reserved-account-keys | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-reward-info | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rpc-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rpc-client-api | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rpc-client-nonce-utils | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-rpc-client-types | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-sanitize | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-sdk | 2.3.1 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-sdk-ids | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-sdk-macro | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-secp256k1-program | 2.2.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-secp256k1-recover | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-secp256r1-program | 2.2.4 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-security-txt | 1.1.3 | MIT OR Apache-2.0 | https://github.com/neodyme-labs/solana-security-txt |
| solana-seed-derivable | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-seed-phrase | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-serde | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-serde-varint | 2.2.2 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-serialize-utils | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-sha256-hasher | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-short-vec | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-shred-version | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-signature | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-signer | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-slot-hashes | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-slot-history | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-stable-layout | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-stake-interface | 1.2.1 | Apache-2.0 | https://github.com/solana-program/stake |
| solana-streamer | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-svm-feature-set | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-system-interface | 1.0.0 | Apache-2.0 | https://github.com/solana-program/system |
| solana-system-transaction | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-sysvar | 2.3.0 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-sysvar-id | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-thin-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-time-utils | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-tls-utils | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-tpu-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-transaction | 2.2.3 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-transaction-context | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-transaction-error | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-transaction-metrics-tracker | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-transaction-status | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-transaction-status-client-types | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-udp-client | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-validator-exit | 2.2.1 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-version | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-vote-interface | 2.2.6 | Apache-2.0 | https://github.com/anza-xyz/solana-sdk |
| solana-zk-sdk | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| solana-zk-token-sdk | 2.3.13 | Apache-2.0 | https://github.com/anza-xyz/agave |
| spin | 0.9.9 | MIT | https://github.com/mvdnes/spin-rs.git |
| spinning_top | 0.3.0 | MIT/Apache-2.0 | https://github.com/rust-osdev/spinning_top |
| spki | 0.7.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats/tree/master/spki |
| spl-associated-token-account | 4.0.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-associated-token-account | 7.0.0 | Apache-2.0 | https://github.com/solana-program/associated-token-account |
| spl-associated-token-account-client | 2.0.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-discriminator | 0.3.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-discriminator | 0.4.1 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-discriminator-derive | 0.2.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-discriminator-syn | 0.2.1 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-elgamal-registry | 0.2.0 | Apache-2.0 | https://github.com/solana-program/token-2022 |
| spl-generic-token | 1.0.1 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-memo | 5.0.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-memo | 6.0.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-pod | 0.3.1 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-pod | 0.5.1 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-program-error | 0.5.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-program-error | 0.7.0 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-program-error-derive | 0.4.1 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-program-error-derive | 0.5.0 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-tlv-account-resolution | 0.10.0 | Apache-2.0 | https://github.com/solana-program/libraries |
| spl-tlv-account-resolution | 0.7.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-token | 6.0.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-token | 8.0.0 | Apache-2.0 | https://github.com/solana-program/token |
| spl-token-2022 | 4.0.1 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-token-2022 | 8.0.1 | Apache-2.0 | https://github.com/solana-program/token-2022 |
| spl-token-confidential-transfer-ciphertext-arithmetic | 0.3.1 | Apache-2.0 | https://github.com/solana-program/token-2022 |
| spl-token-confidential-transfer-proof-extraction | 0.3.0 | Apache-2.0 | https://github.com/solana-program/token-2022 |
| spl-token-confidential-transfer-proof-generation | 0.4.1 | Apache-2.0 | https://github.com/solana-program/token-2022 |
| spl-token-group-interface | 0.3.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-token-group-interface | 0.6.0 | Apache-2.0 | https://github.com/solana-program/token-group |
| spl-token-metadata-interface | 0.4.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-token-metadata-interface | 0.7.0 | Apache-2.0 | https://github.com/solana-program/token-metadata |
| spl-transfer-hook-interface | 0.10.0 | Apache-2.0 | https://github.com/solana-program/transfer-hook |
| spl-transfer-hook-interface | 0.7.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-type-length-value | 0.5.0 | Apache-2.0 | https://github.com/solana-labs/solana-program-library |
| spl-type-length-value | 0.8.0 | Apache-2.0 | https://github.com/solana-program/libraries |
| sqlx | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-core | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-macros | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-macros-core | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-mysql | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-postgres | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| sqlx-sqlite | 0.8.6 | MIT OR Apache-2.0 | https://github.com/launchbadge/sqlx |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | https://github.com/storyyeller/stable_deref_trait |
| stringprep | 0.1.5 | MIT/Apache-2.0 | https://github.com/sfackler/rust-stringprep |
| strsim | 0.11.1 | MIT | https://github.com/rapidfuzz/strsim-rs |
| subtle | 2.6.1 | BSD-3-Clause | https://github.com/dalek-cryptography/subtle |
| syn | 1.0.109 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| syn | 2.0.119 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| syn | 3.0.6 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| sync_wrapper | 1.0.2 | Apache-2.0 | https://github.com/Actyx/sync_wrapper |
| synstructure | 0.12.6 | MIT | https://github.com/mystor/synstructure |
| synstructure | 0.14.0 | MIT | https://github.com/mystor/synstructure |
| termcolor | 1.4.1 | Unlicense OR MIT | https://github.com/BurntSushi/termcolor |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror-impl | 2.0.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thread_local | 1.1.10 | MIT OR Apache-2.0 | https://github.com/Amanieu/thread_local-rs |
| time | 0.3.55 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| time-core | 0.1.9 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| time-macros | 0.2.32 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| tiny-keccak | 2.0.2 | CC0-1.0 |  |
| tinystr | 0.8.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/tinyvec |
| tokio | 1.53.1 | MIT | https://github.com/tokio-rs/tokio |
| tokio-macros | 2.7.2 | MIT | https://github.com/tokio-rs/tokio |
| tokio-rustls | 0.24.1 | MIT/Apache-2.0 | https://github.com/rustls/tokio-rustls |
| tokio-rustls | 0.26.5 | MIT OR Apache-2.0 | https://github.com/rustls/tokio-rustls |
| tokio-stream | 0.1.19 | MIT | https://github.com/tokio-rs/tokio |
| tokio-tungstenite | 0.20.1 | MIT | https://github.com/snapview/tokio-tungstenite |
| tokio-tungstenite | 0.24.0 | MIT | https://github.com/snapview/tokio-tungstenite |
| tokio-util | 0.7.19 | MIT | https://github.com/tokio-rs/tokio |
| toml | 0.5.11 | MIT/Apache-2.0 | https://github.com/toml-rs/toml |
| toml | 0.8.23 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_datetime | 0.6.11 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_edit | 0.25.15+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_parser | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_write | 0.1.2 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| tower | 0.5.3 | MIT | https://github.com/tower-rs/tower |
| tower-http | 0.6.11 | MIT | https://github.com/tower-rs/tower-http |
| tower-layer | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| tower-service | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| tracing | 0.1.44 | MIT | https://github.com/tokio-rs/tracing |
| tracing-attributes | 0.1.31 | MIT | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | https://github.com/tokio-rs/tracing |
| tracing-log | 0.2.0 | MIT | https://github.com/tokio-rs/tracing |
| tracing-serde | 0.2.0 | MIT | https://github.com/tokio-rs/tracing |
| tracing-subscriber | 0.3.23 | MIT | https://github.com/tokio-rs/tracing |
| try-lock | 0.2.5 | MIT | https://github.com/seanmonstar/try-lock |
| tungstenite | 0.20.1 | MIT OR Apache-2.0 | https://github.com/snapview/tungstenite-rs |
| tungstenite | 0.24.0 | MIT OR Apache-2.0 | https://github.com/snapview/tungstenite-rs |
| typenum | 1.20.1 | MIT OR Apache-2.0 | https://github.com/paholg/typenum |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 | https://github.com/servo/unicode-bidi |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-normalization |
| unicode-properties | 0.1.4 | MIT/Apache-2.0 | https://github.com/unicode-rs/unicode-properties |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-width |
| unicode-xid | 0.2.6 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-xid |
| universal-hash | 0.5.1 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| untrusted | 0.9.0 | ISC | https://github.com/briansmith/untrusted |
| uriparse | 0.6.4 | MIT | https://github.com/sgodwincs/uriparse-rs |
| url | 2.5.8 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| utf-8 | 0.7.6 | MIT OR Apache-2.0 | https://github.com/SimonSapin/rust-utf8 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT | https://github.com/hsivonen/utf8_iter |
| uuid | 1.26.1 | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid |
| valuable | 0.1.1 | MIT | https://github.com/tokio-rs/valuable |
| vcpkg | 0.2.15 | MIT/Apache-2.0 | https://github.com/mcgoo/vcpkg-rs |
| version_check | 0.9.5 | MIT/Apache-2.0 | https://github.com/SergioBenitez/version_check |
| walkdir | 2.5.0 | Unlicense/MIT | https://github.com/BurntSushi/walkdir |
| want | 0.3.1 | MIT | https://github.com/seanmonstar/want |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wasi |
| wasi | 0.9.0+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wasi |
| wasip2 | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wasi-rs |
| wasite | 0.1.0 | Apache-2.0 OR BSL-1.0 OR MIT | https://github.com/ardaku/wasite |
| wasm-bindgen | 0.2.128 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen |
| wasm-bindgen-futures | 0.4.78 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/futures |
| wasm-bindgen-macro | 0.2.128 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro |
| wasm-bindgen-macro-support | 0.2.128 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/macro-support |
| wasm-bindgen-shared | 0.2.128 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared |
| web-sys | 0.3.105 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/web-sys |
| web-time | 1.1.0 | MIT OR Apache-2.0 | https://github.com/daxpedda/web-time |
| webpki-root-certs | 1.0.9 | CDLA-Permissive-2.0 | https://github.com/rustls/webpki-roots |
| webpki-roots | 0.24.0 | MPL-2.0 | https://github.com/rustls/webpki-roots |
| webpki-roots | 0.25.4 | MPL-2.0 | https://github.com/rustls/webpki-roots |
| webpki-roots | 0.26.11 | CDLA-Permissive-2.0 | https://github.com/rustls/webpki-roots |
| webpki-roots | 1.0.9 | CDLA-Permissive-2.0 | https://github.com/rustls/webpki-roots |
| whoami | 1.6.1 | Apache-2.0 OR BSL-1.0 OR MIT | https://github.com/ardaku/whoami |
| winapi | 0.3.9 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| winapi-util | 0.1.11 | Unlicense OR MIT | https://github.com/BurntSushi/winapi-util |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.48.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-targets | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_aarch64_gnullvm | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_aarch64_msvc | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_i686_gnu | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_i686_msvc | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_gnu | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_gnullvm | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_msvc | 0.48.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| winnow | 0.7.15 | MIT | https://github.com/winnow-rs/winnow |
| winnow | 1.0.4 | MIT | https://github.com/winnow-rs/winnow |
| wit-bindgen | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wit-bindgen |
| writeable | 0.6.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| x509-parser | 0.14.0 | MIT/Apache-2.0 | https://github.com/rusticata/x509-parser.git |
| yoke | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| yoke-derive | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zerocopy | 0.8.57 | BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/google/zerocopy |
| zerocopy-derive | 0.8.57 | BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/google/zerocopy |
| zerofrom | 0.1.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zerofrom-derive | 0.1.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zeroize | 1.9.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| zeroize_derive | 1.5.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| zerotrie | 0.2.5 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zerovec | 0.11.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zerovec-derive | 0.11.6 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| zlib-rs | 0.6.8 | Zlib | https://github.com/trifectatechfoundation/zlib-rs |
| zmij | 1.0.23 | MIT | https://github.com/dtolnay/zmij |
| zstd | 0.13.3 | MIT | https://github.com/gyscos/zstd-rs |
| zstd-safe | 7.3.0 | BSD-3-Clause | https://github.com/gyscos/zstd-rs |
| zstd-sys | 2.1.0+zstd.1.5.7 | BSD-3-Clause | https://github.com/gyscos/zstd-rs |

## Appendix A — MIT License

```text
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Appendix B — Apache License 2.0

The full text of the Apache License, Version 2.0 is available at
https://www.apache.org/licenses/LICENSE-2.0.txt and is reproduced in the
file `legal/licenses/APACHE-2.0.txt` of this distribution. Summary of
required notices: recipients must receive a copy of the license (§4a);
modified files must carry prominent change notices (§4b); attribution
notices must be retained (§4c); NOTICE files, where present, must be
included (§4d).

## Appendix C — Mozilla Public License 2.0

The full text of the MPL-2.0 is available at
https://www.mozilla.org/en-US/MPL/2.0/ and is reproduced in the file
`legal/licenses/MPL-2.0.txt` of this distribution. The MPL-covered files in
this distribution are the `webpki-roots` crate contents listed in §3.

## Appendix D — Unicode License v3 (summary)

Unicode data files are provided under the Unicode License v3, a permissive
license requiring retention of the copyright notice and disclaimer; full
text: https://www.unicode.org/license.txt.
