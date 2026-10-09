# evidence/audits/ — EXTERNAL items (GAP MAP v2)

This directory holds third-party deliverables. They are **EXTERNAL** by the
map's ACTION vocabulary: they cannot be produced by this repository and MUST
NOT be fabricated.

## Expected files (do not exist yet — intentionally)

| Expected name | Source | Status |
|---|---|---|
| `staking-audit-<firm>-<date>.pdf` | Independent audit of `programs/staking-suite` | NOT_STARTED — engage an auditor |
| `pentest-<firm>-<date>.pdf` | Penetration test of control plane + API | NOT_STARTED — engage a pentest firm |

## Rules

1. File names must carry the real firm name and the report date.
2. Do NOT commit placeholder or sample audit PDFs — a fake audit report is a
   fraud vector and violates the no-fabrication rule.
3. When a report lands here, add a matching entry under `evidence/external/`
   with `"status": "PASSED"`, the firm's reference id, and the scope covered.
4. Until then, every marketing-facing document must treat these as absent:
   `scripts/verify-marketing-claims.sh` fails any "audited" / "pentested"
   claim that cannot point at a file in this directory.
