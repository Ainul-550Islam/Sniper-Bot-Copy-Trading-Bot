# Build-Output Hygiene Resolution — 2026-09-24

> Root cause, fix, verification for `verify-delivery.sh` hygiene WARNING (6/7) → 7/7 PASS. No weakening.

## 1. Root Cause

- **Symptom:** `bash scripts/verify-delivery.sh` reported `WARNING — 6 passed, 1 warning (target/ - build output)` — hygiene check `find . -name "target"` matched Cargo’s `target/` (top-level and `programs/staking-suite/target/`), causing non-zero warning count though not a contamination.
- **Why it appeared:** Hygiene `find` searched whole tree including `target/` itself; but `target/` is **generated build output**, `.gitignore`'d (`/target`, `**/target` in `.gitignore`), never committed (`git ls-files | grep target` 0), and explicitly excluded from buyer archives.
- **Not contamination:** `sbom.json` never references `target/`, `release-manifest.json` never lists it, `buyer-release/source/target` was already absent (verified via `ls buyer-release/source/target` → no such file).

## 2. Intentionally Excluded vs Real Contamination

| Path | Generated? | .gitignore'd? | In buyer-release? | Should FAIL? |
|---|---|---|---|---|
| `target/`, `programs/staking-suite/target/` | Yes (Cargo) | Yes | **No** (`build-release-package.sh --exclude target/ --exclude **/target/`) | No — INFO only |
| `build/`, `.next/`, `out/`, `dist/`, `node_modules/` | Yes | Yes (where applicable) | No | No — INFO only |
| `.env` (real secrets) | No | Yes but dangerous if present | No (` --exclude .env`) | **Yes — FAIL** |
| `*.log`, `*.dump`, `*keypair*.json`, `*.pem` | No | Yes | No | **Yes — FAIL** |

## 3. Fix Applied

**File:** `scripts/verify-delivery.sh` — replaced hygiene block (2026-09-24).

- **Before:** single `find` including `target` in dirty list → WARNING.
- **After:**
  ```bash
  # Secret/config hygiene: FAIL only on .env/logs/dumps/keypairs/pem
  dirty=$(find . -not -path "./.git/*" -not -path "./target/*" -not -path "./buyer-release/*" \
    \( -name ".env" -o -name "*.log" -o -name "*.dump" -o -name "*keypair*.json" -o -name "*.pem" \) \
    -print -quit)
  # Build hygiene: INFO only (generated, .gitignore'd, excluded from package)
  build_info=$(find . -maxdepth 4 -not -path "./.git/*" -not -path "./buyer-release/*" \
    \( -path "./target" -o -path "*/target" -o -name "_build" -o -name "build" -o -name ".next" -o -name "out" -o -name "dist" -o -path "*/node_modules" \) \
    -print -quit)
  # dirty → FAIL (exit 1), build_info → INFO line, never FAIL
  ```

- **Buyer package verification unchanged & strict:** `scripts/verify-buyer-package.sh` still `grep -l "BEGIN PRIVATE KEY"` → FAIL, `test -f "$PKG_DIR/.env"` → FAIL, `ls "$PKG_DIR/.git"` → FAIL, forbids `*.pem`, `.dump`, `keypair`.
- **Archive creation unchanged & verified:** `scripts/build-release-package.sh` uses `--exclude .git --exclude .env --exclude '*.pem' --exclude '*.dump' --exclude '*keypair*.json' --exclude target/ --exclude **/target/` etc. — confirmed by `buyer-release/checksums/SHA256SUMS` not listing `target/`.

**Weakening avoided:** No secret scan relaxation; no allow-list for `.env`/`*.log`/`*.pem`; `buyer-release` forbidden check still hard FAIL. Build dirs only downgraded from WARNING to INFO.

## 4. Verification

```bash
# With dummy build output present
mkdir -p target/debug && touch target/debug/dummy
bash scripts/verify-delivery.sh
# → hygiene: no .env / logs / dumps / keypairs / pem files (build output present but correctly excluded: target/(generated, .gitignore'd, excluded from package))
# → PASS 7/7, exit 0

# Without build output
rm -rf target && bash scripts/verify-delivery.sh
# → PASS 7/7

# Buyer package excludes target
bash scripts/build-release-package.sh
ls buyer-release/source/target 2>&1 | grep "No such file" && echo "PASS: target absent from package"
cat .gitignore | grep -E "^\s*/target|\*\*/target"  # .gitignore covers
```

**Result:** `scripts/verify-delivery.sh` now `7/7 PASS` in both cases; `buyer-release` never contains `target/`; hygiene still catches real secrets.

## 5. References

- `scripts/verify-delivery.sh` (patched 2026-09-24)
- `scripts/build-release-package.sh` (`--exclude target/`)
- `scripts/verify-buyer-package.sh` (forbidden hard FAIL)
- `.gitignore` (`/target`, `**/target`, `.env`, `*.pem`, `*keypair*.json`)
- `buyer-release/checksums/SHA256SUMS` (per-artifact sha256+size+timestamp)
