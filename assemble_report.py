#!/usr/bin/env python3
"""Assemble AUDIT_PASS_REPORT.md: 22-item report skeleton + complete contents
of every file modified/added in the audit pass. Run from /home/user/sniper-suite."""
import os, sys

ROOT = "/home/user/sniper-suite"
OUT = "/home/user/AUDIT_PASS_REPORT.md"

ADDED = [
    "crates/module-polymarket/src/collateral.rs",
]
MODIFIED_CODE = [
    "crates/module-polymarket/src/lib.rs",
    "crates/module-polymarket/src/error.rs",
    "crates/module-polymarket/src/ctf.rs",
    "crates/module-sniper/src/lib.rs",
    "crates/core/src/auth.rs",
    "programs/staking-suite/src/lib.rs",
    "programs/staking-suite/src/state.rs",
    "programs/staking-suite/src/error.rs",
    "programs/staking-suite/src/instruction.rs",
    "programs/staking-suite/src/processor.rs",
    "programs/staking-suite/tests/validator_e2e.rs",
]
MODIFIED_CFG = [
    "config.toml.example",
    "release-manifest.json",
]
MODIFIED_DOCS = [
    "CHANGELOG.md",
    "README.md",
    "AUDIT.md",
    "docs/STAKING.md",
    "docs/MODULES.md",
    "docs/TESTING.md",
    "docs/REPOSITORY-MAP.md",
    "docs/DELIVERY-MANIFEST.md",
    "docs/HANDOVER.md",
    "docs/BUYER-DUE-DILIGENCE.md",
    "docs/BUYER-FAQ.md",
    "docs/BUYER-OVERVIEW.md",
    "docs/BUYER-RISK-REGISTER.md",
    "docs/CAPABILITY-MATRIX.md",
    "docs/DEMO-RUNBOOK.md",
    "docs/EVIDENCE-INDEX.md",
    "docs/FINAL-DELIVERY.md",
    "docs/ACCEPTANCE-CHECKLIST.md",
    "docs/BUYER-DEPLOYMENT.md",
    "docs/BUYER-QUICKSTART.md",
]

LANG = {".rs": "rust", ".toml": "toml", ".json": "json", ".md": "markdown",
        ".sql": "sql", ".sh": "bash", ".example": "toml"}

def fence_for(text: str) -> str:
    f = "```"
    while f in text:
        f += "`"
    return f

def lang_of(path: str) -> str:
    if path.endswith("config.toml.example"):
        return "toml"
    return LANG.get(os.path.splitext(path)[1], "")

def emit(out, section, paths):
    out.write(f"\n## {section}\n")
    for p in paths:
        full = os.path.join(ROOT, p)
        with open(full, "r", encoding="utf-8") as fh:
            text = fh.read()
        if not text.endswith("\n"):
            text += "\n"
        f = fence_for(text)
        lines = text.count("\n")
        size = os.path.getsize(full)
        out.write(f"\n### FILE: `{p}` — complete final content ({lines} lines, {size} bytes)\n\n")
        out.write(f"{f}{lang_of(p)}\n{text}{f}\n")

def main():
    total_files = 0
    with open(OUT, "w", encoding="utf-8") as out:
        out.write("PLACEHOLDER_HEADER\n")
        emit(out, "Appendix A — ADDED file (complete final content)", ADDED)
        emit(out, "Appendix B — MODIFIED source files (complete final content)", MODIFIED_CODE)
        emit(out, "Appendix C — MODIFIED config/manifest files (complete final content)", MODIFIED_CFG)
        emit(out, "Appendix D — MODIFIED documentation files (complete final content)", MODIFIED_DOCS)
    total_files = len(ADDED) + len(MODIFIED_CODE) + len(MODIFIED_CFG) + len(MODIFIED_DOCS)
    print(f"assembled {total_files} files -> {OUT} ({os.path.getsize(OUT)} bytes)")

if __name__ == "__main__":
    main()
