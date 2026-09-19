# Staking program identity status (final buyer-handover package, 2026-09-19)

- Artifact: `staking_suite.so` — 187,504 bytes, SHA-256
  `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`
  (built with agave 2.1.21 / platform-tools v1.43; byte-identical across
  first build, incremental rebuild, and full COLD rebuild — see
  `../evidence/compile-logs/phase3-sbf-determinism-rerun.log`).
- Declared program id in source: `3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy`
  — **PRE-DEPLOYMENT PLACEHOLDER. The program has NOT been deployed to any
  cluster by the vendor. No real program id exists yet; none was invented.**
- The keypair `cargo build-sbf` auto-generates does NOT match the declared
  id (fail-closed by design; documented in `docs/STAKING.md`). Do not
  deploy with it.
- Identity tracking: `scripts/staking-identity.sh` (in `../source-tree/`)
  tracks the declared id across source + 4 documents (README,
  BUYER-DUE-DILIGENCE, STAKING, BUYER-ACCEPTANCE-TEST) and fails `verify`
  if any disagree or go stale after `set-id`.
- Buyer deployment path (HUMAN ACTION): `solana-keygen new -o
  program-keypair.json` → `scripts/staking-identity.sh set-id
  program-keypair.json` → rebuild → `scripts/staking-identity.sh deploy
  --keypair program-keypair.json --url <RPC>`. The script refuses
  keypair≠declare_id mismatches and refuses the placeholder id on public
  clusters.
- This .so supersedes the freeze-era build (`9e113678…`), which is NOT
  included in this package and must not be used for this source. The
  superseded-artifact note is preserved here as history, not as a
  deliverable.
