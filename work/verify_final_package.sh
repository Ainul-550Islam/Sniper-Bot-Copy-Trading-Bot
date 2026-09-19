#!/usr/bin/env bash
# Re-verify the FINAL buyer-handover deliverables after any environment
# re-provision / snapshot restore. Run: bash work/verify_final_package.sh
set -u
FAIL=0
note() { printf '%s\n' "$*"; }
chk()  { if eval "$2"; then note "  OK   $1"; else note "  FAIL $1"; FAIL=1; fi; }

note "== repo tree identity (INFORMATIONAL during forensic cycle — divergence from frozen v0.1 baseline 033b582f… is expected; see docs/SOURCE-OF-TRUTH.md §4) =="
note "  repo files now: $(find /home/user/sniper-suite -type f -not -path '*/target/*' | wc -l) (v0.1 baseline was 183)"
note "  (tree-hash assertion removed for the dev cycle; package checks below remain strict)"

note "== final package =="
P=/home/user/buyer-release-final
chk "package exists with 328 files" "[ \$(find $P -type f | wc -l) -eq 328 ]"
chk "hidden dirs present in mirror" "[ -d $P/source-tree/.cargo ] && [ -d $P/source-tree/.github ] && [ -f $P/source-tree/.dockerignore ]"
chk "exec bits on 3 mirror scripts" \
  "[ -x $P/source-tree/scripts/release-check.sh ] && [ -x $P/source-tree/scripts/verify-delivery.sh ] && [ -x $P/source-tree/scripts/staking-identity.sh ]"
chk "exec bits on 3 deployment/scripts copies" \
  "[ -x $P/deployment/scripts/release-check.sh ] && [ -x $P/deployment/scripts/verify-delivery.sh ] && [ -x $P/deployment/scripts/staking-identity.sh ]"
chk "sha256sum -c all-OK (326 entries post-audit)" "cd $P && sha256sum -c checksums/SHA256SUMS >/dev/null 2>&1"
chk "tarball hash d0ab6a70…" \
  "echo 'd0ab6a70deedf6fa17a8e30f02ab86353a0c282ed329f808a8d75acdf1f28ac9  $P/source/sniper-suite-FINAL-src.tar.gz' | sha256sum -c - >/dev/null 2>&1"
chk ".so hash 57a890fa…" \
  "echo '57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564  $P/binaries/staking_suite.so' | sha256sum -c - >/dev/null 2>&1"

note "== exec bits in live repo (snapshot restore strips them) =="
chk "repo scripts executable" \
  "[ -x /home/user/sniper-suite/scripts/release-check.sh ] && [ -x /home/user/sniper-suite/scripts/verify-delivery.sh ] && [ -x /home/user/sniper-suite/scripts/staking-identity.sh ]"

note "== historical package untouched =="
chk "buyer-release/ still 297 files" "[ \$(find /home/user/buyer-release -type f | wc -l) -eq 297 ]"

if [ $FAIL -eq 0 ]; then note "ALL FINAL-PACKAGE CHECKS PASS"; else note "SOME CHECKS FAILED — repair before any further work"; fi
exit $FAIL
