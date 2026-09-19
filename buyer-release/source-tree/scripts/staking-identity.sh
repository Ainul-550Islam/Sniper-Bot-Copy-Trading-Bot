#!/usr/bin/env bash
# =============================================================================
# staking-identity.sh — staking program identity verification & guarded deploy
# =============================================================================
#
# The staking program's on-chain identity is declared exactly once in source:
#
#     programs/staking-suite/src/lib.rs  ->  declare_id!("<base58 pubkey>")
#
# That declaration is the single source of truth. Every PDA the program
# derives (config, stake accounts, vault, treasury, metadata) is keyed off it,
# and every buyer-facing document quotes it. This script exists so that:
#
#   1. `verify`  — all references agree, and everyone can see whether the id is
#                  still the documented pre-deployment placeholder.
#   2. `set-id`  — when the operator generates the FINAL program keypair, the
#                  declaration and every buyer-facing document are updated
#                  atomically and re-verified. Historical records (AUDIT.md)
#                  are deliberately NOT rewritten — they document the past.
#   3. `deploy`  — deploys target/deploy/staking_suite.so ONLY when the
#                  supplied program keypair's public key equals the declared
#                  id, the binary is up to date with the source, and (for any
#                  non-localhost cluster) the id is no longer the placeholder.
#
# NOTHING here invents an identity: `set-id` requires a real keypair file you
# generated and control; `deploy` refuses to proceed on any mismatch.
#
# Usage:
#   scripts/staking-identity.sh verify
#   scripts/staking-identity.sh set-id <program-keypair.json>
#   scripts/staking-identity.sh deploy --keypair <program-keypair.json> \
#                                      --url <rpc-url> [--skip-rebuild]
#
# Requirements: bash, grep, sed, sha256sum; for set-id/deploy: solana-keygen,
# solana, cargo build-sbf (Agave 2.1.x toolchain) on PATH.
# =============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIB_RS="$REPO_ROOT/programs/staking-suite/src/lib.rs"
SO_PATH="${STAKING_SO:-$REPO_ROOT/programs/staking-suite/target/deploy/staking_suite.so}"

# The id that ships in the source tree before the buyer deploys. Deploying
# under this id to a public cluster is refused (see cmd_deploy). It is a
# syntactically valid base58 pubkey chosen so host tests and PDAs compute
# normally; it does NOT correspond to any keypair held by the vendor.
PLACEHOLDER_ID="3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy"

# Files that quote the program id and MUST track the declaration.
# AUDIT.md is intentionally absent: it is an append-only historical log whose
# §23/§28 entries record the id as it was at the time of writing.
TRACKED_FILES=(
  "README.md"
  "docs/BUYER-DUE-DILIGENCE.md"
  "docs/STAKING.md"
)

die() { echo "ERROR: $*" >&2; exit 1; }
info() { echo "[identity] $*"; }

declared_id() {
  grep -oE 'declare_id!\("[1-9A-HJ-NP-Za-km-z]{32,44}"\)' "$LIB_RS" \
    | head -1 | sed -E 's/declare_id!\("(.*)"\)/\1/'
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "required executable not found: $1 (install the Agave 2.1.x CLI: https://github.com/anza-xyz/agave/releases)"
}

cmd_verify() {
  local id; id="$(declared_id)"
  [ -n "$id" ] || die "could not extract declare_id! from $LIB_RS"
  info "declared program id (source of truth): $id"
  if [ "$id" = "$PLACEHOLDER_ID" ]; then
    info "STATUS: this is the documented PRE-DEPLOYMENT PLACEHOLDER."
    info "        Generate the final program keypair, then run:"
    info "        scripts/staking-identity.sh set-id <program-keypair.json>"
  fi
  local failures=0 f hits
  for f in "${TRACKED_FILES[@]}"; do
    if grep -qF "$id" "$REPO_ROOT/$f"; then
      echo "  OK    $f quotes the declared id"
    else
      echo "  FAIL  $f does NOT quote the declared id"
      failures=$((failures + 1))
    fi
    # any OTHER id-looking declare quotes of the old/placeholder value
    if [ "$id" != "$PLACEHOLDER_ID" ] && grep -qF "$PLACEHOLDER_ID" "$REPO_ROOT/$f"; then
      echo "  FAIL  $f still contains the stale placeholder id"
      failures=$((failures + 1))
    fi
  done
  # repo-wide stale-reference sweep (exclude build dirs + historical audit log)
  local stale
  stale="$(grep -rlF "$PLACEHOLDER_ID" "$REPO_ROOT" \
    --include='*.rs' --include='*.md' --include='*.toml' --include='*.json' \
    --include='*.yml' --include='*.example' --include='*.template' 2>/dev/null \
    | grep -v '/target/' | grep -v '/AUDIT.md$' || true)"
  if [ "$id" != "$PLACEHOLDER_ID" ] && [ -n "$stale" ]; then
    echo "  FAIL  stale placeholder references remain in:"
    echo "$stale" | sed 's/^/          /'
    failures=$((failures + 1))
  fi
  if [ "$failures" -ne 0 ]; then
    die "$failures identity reference failure(s) — fix before deployment"
  fi
  info "verify: all tracked references agree."
}

cmd_set_id() {
  local kp="${1:-}"
  [ -n "$kp" ] || die "usage: staking-identity.sh set-id <program-keypair.json>"
  require_tool solana-keygen
  [ -f "$kp" ] || die "keypair file not found: $kp"
  local new_id old_id
  new_id="$(solana-keygen pubkey "$kp")" || die "solana-keygen could not read $kp"
  old_id="$(declared_id)"
  [ -n "$new_id" ] || die "empty pubkey from keypair"
  [ "$new_id" != "$old_id" ] || die "declared id already equals this keypair's pubkey; nothing to do"
  info "old declared id: $old_id"
  info "new declared id: $new_id  (from $kp)"
  sed -i "s/declare_id!(\"$old_id\")/declare_id!(\"$new_id\")/" "$LIB_RS"
  grep -qF "declare_id!(\"$new_id\")" "$LIB_RS" || die "failed to update $LIB_RS"
  local f
  for f in "${TRACKED_FILES[@]}"; do
    if grep -qF "$old_id" "$REPO_ROOT/$f"; then
      sed -i "s/$old_id/$new_id/g" "$REPO_ROOT/$f"
      info "updated $f"
    fi
  done
  info "NOTE: AUDIT.md keeps the historical id by design (append-only log)."
  info "NOTE: the .so must be rebuilt for the new id (cargo build-sbf) — deploy does this automatically."
  cmd_verify
  info "set-id complete. Rebuild + deploy with:"
  info "  scripts/staking-identity.sh deploy --keypair $kp --url <rpc-url>"
}

so_is_fresh() {
  [ -f "$SO_PATH" ] || return 1
  local newer
  newer="$(find "$REPO_ROOT/programs/staking-suite/src" "$REPO_ROOT/programs/staking-suite/Cargo.toml" "$REPO_ROOT/programs/staking-suite/Cargo.lock" \
    -newer "$SO_PATH" -type f 2>/dev/null | head -1)"
  [ -z "$newer" ]
}

cmd_deploy() {
  local kp="" url="" skip_rebuild=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --keypair) kp="${2:-}"; shift 2 ;;
      --url) url="${2:-}"; shift 2 ;;
      --skip-rebuild) skip_rebuild=1; shift ;;
      *) die "unknown deploy argument: $1" ;;
    esac
  done
  [ -n "$kp" ] && [ -n "$url" ] || die "usage: staking-identity.sh deploy --keypair <program-keypair.json> --url <rpc-url> [--skip-rebuild]"
  require_tool solana-keygen; require_tool solana
  [ -f "$kp" ] || die "keypair file not found: $kp"
  cmd_verify
  local kp_id id
  kp_id="$(solana-keygen pubkey "$kp")"
  id="$(declared_id)"
  [ "$kp_id" = "$id" ] || die "keypair pubkey ($kp_id) != declared program id ($id). Refusing to deploy: PDAs would derive from the declared id while the program lives at a different address."
  case "$url" in
    *localhost*|*127.0.0.1*) : ;;
    *)
      if [ "$id" = "$PLACEHOLDER_ID" ]; then
        die "refusing to deploy the PLACEHOLDER id to a public cluster ($url). Generate the final keypair and run set-id first."
      fi
      ;;
  esac
  if [ "$skip_rebuild" -eq 0 ]; then
    if so_is_fresh; then
      info ".so is newer than all program sources — reusing $SO_PATH"
    else
      info "rebuilding .so from current source (cargo build-sbf)…"
      ( cd "$REPO_ROOT/programs/staking-suite" && cargo build-sbf )
    fi
  fi
  [ -f "$SO_PATH" ] || die "no program binary at $SO_PATH (run cargo build-sbf)"
  info "binary: $SO_PATH"
  sha256sum "$SO_PATH"
  info "deploying to $url as $id …"
  solana program deploy --program-id "$kp" --url "$url" "$SO_PATH"
  info "deploy command succeeded. Verifying on-chain account…"
  solana account "$id" --url "$url" | sed 's/^/[identity] /'
  info "DEPLOY COMPLETE. Record the transaction signature from the output above,"
  info "then re-run: scripts/staking-identity.sh verify"
}

main() {
  local sub="${1:-}"
  shift || true
  case "$sub" in
    verify)  cmd_verify ;;
    set-id)  cmd_set_id "$@" ;;
    deploy)  cmd_deploy "$@" ;;
    ""|-h|--help|help)
      sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      ;;
    *) die "unknown subcommand: $sub (expected: verify | set-id | deploy)" ;;
  esac
}

main "$@"
