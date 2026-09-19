# Staking program identity status (at packaging time)

- Artifact: `staking_suite.so` — 187,504 bytes, SHA-256
  `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`
  (built with agave 2.1.21 / platform-tools v1.43; byte-identical rebuild
  verified — see `../evidence/build/phase3-sbf-rebuild.log`).
- Declared program id in source: `3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy`
  — **PRE-DEPLOYMENT PLACEHOLDER. The program has NOT been deployed to any
  cluster by the vendor. No real program id exists yet; none was invented.**
- The keypair `cargo build-sbf` auto-generates does NOT match the declared
  id (fail-closed; documented in `docs/STAKING.md`). Do not deploy with it.
- Buyer deployment path (HUMAN ACTION): `solana-keygen new -o
  program-keypair.json` → `scripts/staking-identity.sh set-id
  program-keypair.json` → rebuild → `scripts/staking-identity.sh deploy
  --keypair program-keypair.json --url <RPC>`. The script refuses
  keypair≠declare_id mismatches and placeholder ids on public clusters.
- This .so supersedes the freeze-era build (`9e113678…`), which is NOT
  included in this package and must not be used for this source.
