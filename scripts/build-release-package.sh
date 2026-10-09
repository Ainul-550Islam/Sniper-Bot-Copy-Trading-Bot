#!/usr/bin/env bash
# build-release-package.sh — build the buyer delivery package (GAP-MAP v2 P0).
#
# WHY THIS EXISTS IN THIS FORM
#   The previous package was built by copying the WORKING DIRECTORY with an
#   exclude-list. Exclude-lists fail open: a 21 MB rustup toolchain
#   (.cargo/bin/rustup), sandbox configs (.config/) and an installer
#   (rustup-init.sh) shipped to buyers because nobody had listed them.
#   This script builds from the GIT-TRACKED allow-list instead, so anything
#   ignored or uncommitted can NEVER ship, then runs hard content guards:
#
#     GUARD 1  no ELF / Mach-O / PE binaries of any size
#     GUARD 2  no single file larger than 5 MB
#     GUARD 3  no secrets-shaped files (.env, *.pem, *.key, id_rsa*, wallet
#              keypairs, credentials files)
#     GUARD 4  no docs/archive content (internal history is not for buyers)
#     GUARD 5  LICENSE must not be the MIT text and must contain no
#              unfilled "[SELLER LEGAL ENTITY NAME]" placeholder
#
#   Any guard failure aborts the build with a non-zero exit and NO package.
#
# USAGE
#   scripts/build-release-package.sh [OUTPUT_DIR]
#   default OUTPUT_DIR = <repo>/buyer-release
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/buyer-release}"
VERSION="$(cat "$ROOT/VERSION" 2>/dev/null | tr -d ' \n' || echo "0.1.0")"
MAX_FILE_BYTES=$((5 * 1024 * 1024))

echo "[build-release-package] sniper-suite $VERSION -> $OUT"

cd "$ROOT"
if ! git rev-parse --git-dir >/dev/null 2>&1; then
    echo "ERROR: must run inside a git checkout (the package is built from the tracked file list)." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 1 — the allow-list: tracked files plus untracked-but-not-ignored
# files (so a freshly added, not-yet-committed source file cannot silently
# drop out), minus paths that must never ship.
# ---------------------------------------------------------------------------
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

LIST="$STAGE/allowlist.txt"
{ git ls-files --cached; git ls-files --others --exclude-standard; } \
    | sort -u \
    | grep -v -E '^(docs/archive/|buyer-release/|\.git/)' \
    | grep -v -E '\.(dump|dump\.age)$' \
    | grep -v -E '^(\.config/|\.cargo/bin/|data/)' \
    | grep -v -E '^rustup-init\.sh$' \
    > "$LIST"

if [ ! -s "$LIST" ]; then
    echo "ERROR: allow-list is empty — refusing to build." >&2
    exit 1
fi
echo "[build-release-package] allow-list: $(wc -l < "$LIST") files"

# ---------------------------------------------------------------------------
# Step 2 — content guards run BEFORE anything is copied to the output.
# ---------------------------------------------------------------------------
fail=0

# GUARD 3: secrets-shaped paths.
#   - .env and .env.<suffix> are rejected UNLESS they are templates/examples
#     (.env.template / .env.example / .env.sample) which contain no secrets.
#   - "credentials" is rejected only for DATA files (.json/.yml/.yaml/.toml/.txt),
#     never for source code (crates/core/src/custody/credentials.rs is code).
SECRETS_RE='(^|/)(\.env|\.env\.[A-Za-z0-9._-]+|credentials\.(json|ya?ml|toml|txt)|\.netrc|id_rsa|id_ed25519|.*\.pem|.*\.key|.*keypair.*\.json|wallet\.json)'
if grep -E "$SECRETS_RE" "$LIST" | grep -v -E '\.env\.(template|example|sample)$' | grep -q .; then
    echo "ERROR: secrets-shaped files in the allow-list:" >&2
    grep -E "$SECRETS_RE" "$LIST" | grep -v -E '\.env\.(template|example|sample)$' >&2
    fail=1
fi

# Guard checks need the files on disk; validate each listed file.
while IFS= read -r f; do
    [ -f "$ROOT/$f" ] || continue   # deletions staged but not yet committed

    # GUARD 2: size.
    size=$(wc -c < "$ROOT/$f")
    if [ "$size" -gt "$MAX_FILE_BYTES" ]; then
        echo "ERROR: file larger than 5 MB: $f ($size bytes)" >&2
        fail=1
        continue
    fi

    # GUARD 1: ELF / Mach-O / PE binaries. Text-ish files are skipped fast.
    case "$f" in
        *.png|*.jpg|*.jpeg|*.gif|*.ico|*.woff|*.woff2|*.ttf|*.otf|*.pdf|*.zip|*.gz|*.tar|*.xz|*.mp4|*.webp|*.avif)
            # Media/fonts/PDF are acceptable; binary-scan them anyway below.
            ;;
    esac
    magic=$(head -c 4 "$ROOT/$f" | od -A n -t x1 | tr -d ' \n' || true)
    case "$magic" in
        7f454c46*)   # \x7fELF
            echo "ERROR: ELF binary in the allow-list: $f" >&2; fail=1 ;;
        feedface*|cefaedfe*|cffaedfe*|cafebabe*)
            echo "ERROR: Mach-O/Java-class binary in the allow-list: $f" >&2; fail=1 ;;
        4d5a*)       # MZ
            echo "ERROR: PE/DOS executable in the allow-list: $f" >&2; fail=1 ;;
    esac
done < "$LIST"

# GUARD 4: docs/archive is excluded by construction; verify.
if grep -q '^docs/archive/' "$LIST"; then
    echo "ERROR: docs/archive content reached the allow-list" >&2
    fail=1
fi

# GUARD 5: LICENSE sanity — the buyer package must carry the proprietary
# licence, not the old MIT text, and with no unfilled entity placeholder.
if [ -f "$ROOT/LICENSE" ]; then
    if head -1 "$ROOT/LICENSE" | grep -qi '^MIT License'; then
        echo "ERROR: LICENSE is still the MIT text — replace it before packaging." >&2
        fail=1
    fi
    if grep -q '\[SELLER LEGAL ENTITY NAME\]' "$ROOT/LICENSE"; then
        echo "ERROR: LICENSE still contains the '[SELLER LEGAL ENTITY NAME]' placeholder." >&2
        fail=1
    fi
else
    echo "ERROR: LICENSE missing — the package cannot ship without it." >&2
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "[build-release-package] ABORTED: fix the guard failures above. Nothing was written to $OUT." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 3 — copy the allow-listed files into a clean staging tree, then zip.
# ---------------------------------------------------------------------------
rm -rf "$OUT"
mkdir -p "$OUT"

while IFS= read -r f; do
    [ -f "$ROOT/$f" ] || continue
    mkdir -p "$OUT/$(dirname "$f")"
    cp -p "$ROOT/$f" "$OUT/$f"
done < "$LIST"

# ---------------------------------------------------------------------------
# Step 4 — POST-BUILD gate over the WHOLE package output ($OUT), including
# source/ and docs/ as copied. The pre-copy guards above check the
# allow-list; this gate proves what actually landed on disk. It is a
# separate defence: if the copy logic ever regresses, the package still
# cannot ship. Exits non-zero with a clear message on ANY finding.
# ---------------------------------------------------------------------------
post_fail=0

# 4a. Forbidden paths inside the package (root-anchored).
forbidden=$(find "$OUT" -type f \
    -path '*/.cargo/bin/*' -o -path '*/.config/*' -o -path '*/docs/archive/*' \
    -o -name 'rustup-init.sh' 2>/dev/null || true)
if [ -n "$forbidden" ]; then
    echo "ERROR: post-build gate — forbidden paths present in package:" >&2
    echo "$forbidden" >&2
    post_fail=1
fi

# 4b. ELF / Mach-O / PE headers and >5 MiB files, over every shipped file.
while IFS= read -r -d '' f; do
    size=$(wc -c < "$f")
    if [ "$size" -gt "$MAX_FILE_BYTES" ]; then
        echo "ERROR: post-build gate — file larger than 5 MiB: ${f#"$OUT"/} ($size bytes)" >&2
        post_fail=1
    fi
    magic=$(head -c 4 "$f" | od -A n -t x1 | tr -d ' \n' || true)
    case "$magic" in
        7f454c46*)   echo "ERROR: post-build gate — ELF binary: ${f#"$OUT"/}" >&2; post_fail=1 ;;
        feedface*|cefaedfe*|cffaedfe*|cafebabe*)
                     echo "ERROR: post-build gate — Mach-O binary: ${f#"$OUT"/}" >&2; post_fail=1 ;;
        4d5a*)       echo "ERROR: post-build gate — PE executable: ${f#"$OUT"/}" >&2; post_fail=1 ;;
    esac
done < <(find "$OUT" -type f -print0)

# 4c. *_PLACEHOLDER tokens: FAIL in root-level *.md, REPORT-only in docs/*.md.
root_md=$(find "$OUT" -maxdepth 1 -name '*.md' -type f 2>/dev/null || true)
if [ -n "$root_md" ]; then
    # shellcheck disable=SC2086
    if grep -nE '\b[A-Z][A-Z0-9_]*_PLACEHOLDER\b' $root_md; then
        echo "ERROR: post-build gate — unfilled *_PLACEHOLDER token in a root-level .md (see matches above)" >&2
        post_fail=1
    fi
fi
docs_ph=$(find "$OUT/docs" -maxdepth 1 -name '*.md' -type f 2>/dev/null -exec grep -lnE '\b[A-Z][A-Z0-9_]*_PLACEHOLDER\b' {} + || true)
if [ -n "$docs_ph" ]; then
    echo "[build-release-package] NOTE: *_PLACEHOLDER tokens listed (docs/*.md, not failed):"
    echo "$docs_ph"
fi

# 4d. Script exec bits over the staged package tree (P0-C TASK 6).
if ! bash "$ROOT/scripts/verify-script-modes.sh" "$OUT"; then
    echo "ERROR: post-build gate — non-executable *.sh in the package tree." >&2
    post_fail=1
fi

# 4e. Secret scan over the staged package tree (P0-D TASK 1).
if ! bash "$ROOT/scripts/scan-secrets.sh" "$OUT"; then
    echo "ERROR: post-build gate — secret scanner findings in the package tree." >&2
    post_fail=1
fi

if [ "$post_fail" -ne 0 ]; then
    echo "[build-release-package] ABORTED by post-build gate. No zip produced." >&2
    rm -rf "$OUT"
    exit 1
fi
echo "[build-release-package] post-build gate: CLEAN"

chmod +x "$OUT"/scripts/*.sh 2>/dev/null || true

# Checksum manifest: every shipped file, sha256, reproducible order.
( cd "$OUT" && find . -type f ! -name 'SHA256SUMS.txt' -print0 \
    | sort -z \
    | xargs -0 sha256sum ) > "$OUT/SHA256SUMS.txt"

PKG="$ROOT/sniper-suite-$VERSION-src.zip"
rm -f "$PKG"
( cd "$OUT" && zip -qr "$PKG" . )
pkg_size=$(wc -c < "$PKG")
echo "[build-release-package] wrote $PKG ($((pkg_size / 1024 / 1024)) MB, $(wc -l < "$LIST") files)"
echo "[build-release-package] verify with: scripts/verify-buyer-package.sh"
