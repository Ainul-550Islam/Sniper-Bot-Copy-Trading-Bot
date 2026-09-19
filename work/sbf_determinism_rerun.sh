#!/usr/bin/env bash
# Re-run of the SBF determinism check with a COMPLETE persisted log.
# v2 fixes: (a) non-login shell had no cargo on PATH; (b) the workspace
# snapshot mechanism wiped programs/staking-suite/target/ between turns
# ("target" is an excluded dir name), so this is now a FULL COLD rebuild:
# fresh platform-tools download + cold sbf cache, compared byte-for-byte
# against the preserved first-build artifact evidence/staking_suite.so.first
# (sha256 57a890fa...). A cold-rebuild match is STRONGER determinism evidence
# than the original incremental touch-rebuild.
set -u -o pipefail
LOG=/home/user/evidence/phase3-sbf-determinism-rerun.log
exec >"$LOG" 2>&1

export RUSTUP_HOME="$HOME/.rustup"
export CARGO_HOME="$HOME/.cargo"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS=1

echo "=== SBF determinism re-run (cold) $(date -u +%FT%TZ) ==="
echo "--- NOTE: workspace snapshot cap wiped ~/.rustup, ~/.cargo/registry and both target/ dirs between turns; re-provisioning toolchain now. All persisted evidence (logs/hashes/package) is intact."
df -h / | tail -1

if ! command -v cargo >/dev/null 2>&1; then
    echo "--- reinstalling rust toolchain 1.98.1 (pinned by rust-toolchain.toml)"
    rustup toolchain install 1.98.1 --profile minimal --no-self-update
    rustup default 1.98.1
fi
# Self-heal rustup proxies: snapshot restore can wipe the ~/.cargo/bin symlinks
# (observed 2026-09-19: only the rustup binary survived; cargo/rustc proxies gone).
if [ -x "$HOME/.cargo/bin/rustup" ] && [ ! -e "$HOME/.cargo/bin/cargo" ]; then
    echo "--- recreating rustup proxy symlinks in ~/.cargo/bin"
    ( cd "$HOME/.cargo/bin" && for p in cargo rustc rustdoc rustfmt cargo-fmt clippy-driver cargo-clippy; do ln -sf rustup "$p"; done )
fi
rustup component add rustfmt clippy --toolchain 1.98.1-x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
cargo --version
rustc --version

EXPECTED_TARBALL_SHA=5da3359e296f1e6c13522b874317cb4cfe33e73301a9dc60b1e1435cc133b397
AGAVE_DIR=/home/user/work/solana-release

if [ ! -x "$AGAVE_DIR/bin/cargo-build-sbf" ]; then
    echo "--- installing agave v2.1.21 from official GitHub release"
    curl -sSL -o /tmp/agave.tar.bz2 \
        https://github.com/anza-xyz/agave/releases/download/v2.1.21/solana-release-x86_64-unknown-linux-gnu.tar.bz2
    GOT_SHA=$(sha256sum /tmp/agave.tar.bz2 | awk '{print $1}')
    echo "tarball sha256: $GOT_SHA (expected $EXPECTED_TARBALL_SHA)"
    if [ "$GOT_SHA" != "$EXPECTED_TARBALL_SHA" ]; then
        echo "TARBALL_HASH_MISMATCH — ABORT"; exit 1
    fi
    mkdir -p "$AGAVE_DIR"
    tar xjf /tmp/agave.tar.bz2 -C "$AGAVE_DIR" --strip-components=1
    rm -f /tmp/agave.tar.bz2
fi
export PATH="$AGAVE_DIR/bin:$PATH"
solana --version

echo "--- preserved first-build artifacts (expected sha 57a890fa...):"
sha256sum /home/user/evidence/staking_suite.so.first
sha256sum /home/user/buyer-release/binaries/staking_suite.so

cd /home/user/sniper-suite/programs/staking-suite
echo "--- COLD cargo build-sbf (platform-tools will auto-download to ~/.cache/solana; sbf target cache is empty)"
cargo build-sbf
RC=$?
echo "REBUILD_EXIT=$RC"
if [ "$RC" -ne 0 ]; then echo "BUILD_FAILED"; exit "$RC"; fi

echo "--- .so AFTER cold rebuild:"
ls -l target/deploy/staking_suite.so
sha256sum target/deploy/staking_suite.so

if cmp -s target/deploy/staking_suite.so /home/user/evidence/staking_suite.so.first; then
    echo "BYTE_IDENTICAL=YES (cold-rebuilt .so == evidence/staking_suite.so.first == package binaries/staking_suite.so)"
else
    echo "BYTE_IDENTICAL=NO — MISMATCH, investigate before claiming determinism"
fi
df -h / | tail -1
echo "=== done $(date -u +%FT%TZ) ==="
