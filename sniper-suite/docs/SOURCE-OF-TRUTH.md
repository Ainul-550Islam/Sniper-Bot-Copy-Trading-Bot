# Source of truth (Phase 1, 2026-09-19)

## 1. Canonical repository

**The one and only canonical source tree is `/home/user/sniper-suite/`.**

* Identity at the start of this forensic cycle (v0.1 handover baseline,
  frozen and packaged): 183 files / 3,293,320 bytes / 86,576 lines,
  tree SHA-256
  `033b582f79c5e1d24a301895fa98d37fae1ed01de15f0052a4b2777a6dd06946`
  (algorithm + reproduction: `buyer-release-final/provenance/SOURCE-PROVENANCE.md`).
* This forensic cycle (Phase 0 onward) edits the repo; the tree hash
  therefore advances. The authoritative live identity is always
  `evidence/source-inventory.{csv,json}` (regenerated on every tree
  change) — never a number quoted in prose.
* There is **no `.git`** in this environment; history is the append-only
  `AUDIT.md`, `CHANGELOG.md`, `evidence/ledger.csv` and the dated pass
  reports. No git-based claim is made anywhere.

## 2. Directories that are NEVER source (do not edit, do not ship as source)

| Path | What it is | Rule |
|---|---|---|
| `buyer-release-final/` | Frozen, checksum-verified **v0.1 buyer package** (328 files; `sha256sum -c` 326/326; its `source-tree/` mirrors the 183-file baseline above) | Read-only history. Editing it invalidates its SHA256SUMS. Re-packaging at the end of this cycle produces a NEW package; this one stays as the labeled v0.1 snapshot. |
| `buyer-release/` | Superseded hardening-pass package (297 files) | Read-only labeled history. |
| `delivery/` | Freeze-era 170-file convenience snapshot (own `ARTIFACTS-NOTE.txt`) | Read-only labeled history. |
| `evidence/` | Vendor-side originals of logs, dumps, inventories, ledger, provenance, SBOM, scans | Evidence only — append/preserve, never treat as product source. |
| `work/` | Scratch: extracted `solana-release/` toolchain remnant (71.7 MB), downloaded protocol IDLs, Raydium source extracts, audit logs, helper scripts | Temporary. Protocol references here were used to hand-build wire layouts; nothing is vendored from here into the repo. |
| `/home/user/*.{md,py,log,sh}` | Vendor pass reports + tooling (`doccheck.py`) + stray gate logs + `rustup-init.sh` | Vendor-side; not product source. |
| `.config/solana/` | solana-cli config remnant | Temporary. |

## 3. Duplicate policy

All duplicate content in the workspace is **intentional packaging/evidence
co-location** (verified by hash-group analysis in
`docs/FORENSIC-FILE-INVENTORY.md` §1/§4): package `source-tree/` mirrors,
`docs/` copies inside packages, `staking_suite.so` ×5 (two packages ×
two locations + evidence original), evidence copies inside packages. The
canonical repo itself contains **zero** intra-repo duplicates. When a file
exists in several places, the precedence order is:

1. `sniper-suite/` (canonical, live)
2. `buyer-release-final/source-tree/` (frozen v0.1 snapshot)
3. everything else (historical/scratch)

## 4. Cycle and re-packaging discipline

* v0.1 buyer-handover baseline = tree `033b582f…` = exactly what
  `buyer-release-final/` contains. That package remains independently
  verifiable (`cd buyer-release-final && sha256sum -c checksums/SHA256SUMS`)
  regardless of later repo edits.
* This forensic/engineering cycle is v0.1.0+ development (CHANGELOG
  `[Unreleased]`). At cycle end the full packaging chain re-runs
  (inventory → fixed-point counts → mirror → tarball → four-way →
  manifest → provenance → SUMS) producing a new final package; the current
  one is then labeled superseded — preserved, never deleted.
* `work/verify_final_package.sh` checks the frozen v0.1 package integrity;
  its repo-tree check is informational during this cycle (divergence from
  `033b582f…` is expected and recorded here).
