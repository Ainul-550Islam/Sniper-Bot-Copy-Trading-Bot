#!/usr/bin/env bash
# verify-marketing-claims.sh — reject unsupported marketing claims
# (PROMPT 5 §M, "MARKETING CLAIM VALIDATION").
#
# Evidence levels (weak → strong):
#   CODE < UNIT_TEST < INTEGRATION_TEST < LIVE_TEST < FUNDED_TEST < EXTERNAL_AUDIT
#
# Rules enforced:
#   1. Banned phrases (the spec list) are rejected wherever they appear
#      in marketing-facing files, UNLESS the very line carries an inline
#      evidence tag naming the evidence file and its level:
#        <!-- evidence:LEVEL path/to/evidence-file.md -->
#      and that file EXISTS and declares `EVIDENCE-LEVEL: <LEVEL>` at
#      least as strong as the phrase requires.
#   2. No claim may cite a higher level than the evidence file declares.
#   3. The `## UNSUPPORTED CLAIMS` section of docs/MARKETING-CLAIMS.md is
#      exempt — listing what must NOT be claimed is its purpose.
#
# Exit 0 = all claims evidence-based; exit 1 = violations (see report).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

exec python3 - "$ROOT" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])

LEVELS = ["CODE", "UNIT_TEST", "INTEGRATION_TEST", "LIVE_TEST", "FUNDED_TEST", "EXTERNAL_AUDIT"]
LEVEL_RANK = {name: i for i, name in enumerate(LEVELS)}

# Banned phrase -> minimum evidence level that could ever justify it.
# None = no evidence level justifies the claim (reject always).
BANNED = {
    "guaranteed": None,
    "risk-free": None,
    "risk free": None,
    "profitable": None,
    "guaranteed profit": None,
    "under 1 second guaranteed": "LIVE_TEST",
    "fully audited": "EXTERNAL_AUDIT",
    "fully isolated": "EXTERNAL_AUDIT",
    "all raydium": "LIVE_TEST",
    "latest polymarket v3": "LIVE_TEST",
    "vault/kms/hsm included": "LIVE_TEST",
    "self-service billing": "LIVE_TEST",
    "self-serve billing": "LIVE_TEST",
    "institutional sla": "EXTERNAL_AUDIT",
    "mainnet proven": "FUNDED_TEST",
}

# Marketing-facing files scanned for banned phrases. The claims registry
# itself is handled specially (SAFE section scanned, UNSUPPORTED exempt).
# Each entry: (file, sections whose lines are EXEMPT because naming
# banned phrases is their documented purpose — claim registries and the
# business matrix's unsafe-claims column). Everything else in a scanned
# file is checked fail-closed. The business matrix's SAFE-claims column
# is gated column-aware by tests/business/business-matrix-completeness.sh
# (a line scanner cannot distinguish table columns).
SCAN_FILES = [
    ("README.md", ()),
    ("docs/BUYER-OVERVIEW.md", ()),
    ("docs/MARKETING-CLAIMS.md", ("UNSUPPORTED CLAIMS",)),
    ("docs/CUSTOMER-SaaS-STATUS-2026.md", ()),
    ("docs/POLYMARKET-COMPATIBILITY-2026.md", ()),
    ("docs/CUSTODY-STATUS-2026.md", ()),
    ("docs/BILLING-STATUS-2026.md", ()),
    ("docs/CURRENT-STATE.md", ()),
    ("docs/CURRENT-BUYER-FACTSHEET-2026.md", ()),
    ("docs/CURRENT-MARKETING-CLAIMS-2026.md", ("UNSUPPORTED claims (banned; the gate rejects them)",)),
    ("docs/CURRENT-COMMERCIAL-GAP-REGISTER-2026.md", ("Pricing honesty",)),
    ("docs/BUSINESS-MATRIX-2026.md", ("The measured matrix", "The full business matrix (11 columns, spec-exact)")),
    ("docs/BUYER-PACKAGE-CONTENTS-2026.md", ()),
]

EVIDENCE_TAG = re.compile(r"<!--\s*evidence:([A-Z_]+)\s+(\S+)\s*-->")
EVIDENCE_DECL = re.compile(r"EVIDENCE-LEVEL:\s*([A-Z_]+)")

def declared_level(path: Path):
    m = EVIDENCE_DECL.search(path.read_text(errors="replace"))
    return m.group(1) if m else None

def evidence_ok(required: str | None, cited_level: str, cited_file: str) -> tuple[bool, str]:
    if required is None:
        return False, "no evidence level can justify this claim (banned outright)"
    if cited_level not in LEVEL_RANK:
        return False, f"unknown evidence level '{cited_level}'"
    if LEVEL_RANK[cited_level] < LEVEL_RANK[required]:
        return False, f"evidence level {cited_level} is weaker than the required {required}"
    p = root / cited_file
    if not p.is_file():
        return False, f"evidence file '{cited_file}' does not exist"
    have = declared_level(p)
    if have is None:
        return False, f"evidence file '{cited_file}' declares no EVIDENCE-LEVEL"
    if LEVEL_RANK[have] < LEVEL_RANK[cited_level]:
        return False, (
            f"evidence file '{cited_file}' declares {have}, weaker than the cited {cited_level}"
        )
    return True, f"justified by {cited_file} ({cited_level})"

def section_spans(text: str):
    """Map heading -> (start, end) line indexes for '## ' sections."""
    spans = {}
    current, start = None, None
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if line.startswith("## "):
            if current is not None:
                spans[current] = (start, i)
            current, start = line[3:].strip(), i + 1
    if current is not None:
        spans[current] = (start, len(lines))
    return spans, lines

violations = []

for rel, exempt_sections in SCAN_FILES:
    p = root / rel
    if not p.is_file():
        continue
    text = p.read_text(errors="replace")
    if exempt_sections:
        spans, lines = section_spans(text)
        scan_ranges = [spans["SAFE CLAIMS"]] if "SAFE CLAIMS" in spans else []
        # Lines outside any known section: still scanned (fail closed).
        covered = set()
        for name in ("SAFE CLAIMS",) + tuple(exempt_sections):
            if name in spans:
                covered.update(range(*spans[name]))
        scan_ranges.append(
            (0, len(lines))
            if not covered
            else None
        )
        checked = []
        for r in scan_ranges:
            if r:
                checked.extend(range(*r))
        checked = sorted(set(i for i in checked if i not in covered and i < len(lines)))
        line_iter = ((i, lines[i]) for i in checked)
    else:
        line_iter = enumerate(text.splitlines())

    for idx, line in line_iter:
        low = line.lower()
        for phrase, required in BANNED.items():
            if phrase not in low:
                continue
            tag = EVIDENCE_TAG.search(line)
            if not tag:
                violations.append(
                    (rel, idx + 1, phrase, "no inline evidence tag on the claim line")
                )
                continue
            ok, why = evidence_ok(required, tag.group(1), tag.group(2))
            if not ok:
                violations.append((rel, idx + 1, phrase, why))

if violations:
    print("[claims] REJECTED — marketing claims exceed their evidence:")
    for rel, line_no, phrase, why in violations:
        print(f"  {rel}:{line_no}  '{phrase}'  — {why}")
    sys.exit(1)

print("[claims] OK — every scanned claim is either clean or evidence-backed")
print("[claims] evidence levels: " + " < ".join(LEVELS))
PY
