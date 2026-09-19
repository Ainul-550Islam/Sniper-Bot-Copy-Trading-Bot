# Independent adversarial audit logs (2026-09-19)

Zero-trust re-verification of the final buyer-handover pass (23-rule
protocol: inventory, source-change diff, test-evidence validity, package
byte verification, secret/hidden-file scan, exec-bit audit, checksum
second-method recompute, tree-hash independent reproduction, archive
rebuild, cross-doc consistency, status semantics, claim→evidence traces,
program-identity + set-id fail-closed observation, money-path adversarial
grep, negative-test mapping, dependency/toolchain pins, stale-claim scan,
package hierarchy, buyer-handover path, IP snapshot).

**Timestamp/hash caveat (honesty note):** these logs were produced BEFORE
the audit's own documentation fixes (D-A1…D-A5) were applied. Hashes/counts
recorded inside the logs (tree `2da6579b…`, 183 files / 3,288,174 B /
86,516 lines, archive `22021e34…`) refer to the pre-fix state of the final
tree. The fixes changed ONLY markdown documents (+ restored exec bits,
which are content-hash-neutral); zero Rust changes. Authoritative post-fix
identity: tree `033b582f79c5e1d24a301895fa98d37fae1ed01de15f0052a4b2777a6dd06946`,
183 files / 3,293,320 B / 86,576 lines — see `provenance/SOURCE-PROVENANCE.json`,
`evidence/inventory/source-inventory.{csv,json}`, and
`docs/FINAL-RELEASE-AUDIT.md` §H (in source-tree/docs/ and docs/).

Findings summary: 5 documentation/integrity defects (D-A1 exec bits
stripped by the sandbox snapshot mechanism; D-A2 incomplete evidence
citation + unlabeled pre-fix FAILED logs; D-A3 release-check tree
qualification; D-A4 stale completeness bullet; D-A5 historical byte-count
nuance) — all fixed. Zero Rust defects, zero secrets, zero content
mismatches, zero phantom checksum entries. Fresh executions during the
audit: verify-delivery 7/7 PASS on the final tree; archive rebuild
byte-identical; SUMS 312/312 by two methods; four-way 0 diffs.
