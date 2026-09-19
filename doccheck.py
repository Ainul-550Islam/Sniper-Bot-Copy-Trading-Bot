#!/usr/bin/env python3
"""Documentation consistency checker for the sniper-suite buyer package.

Checks (repo-root relative):
  1. Every relative markdown link target exists (all .md files in repo).
  2. No zero-width / bidi control characters.
  3. Version references: '0.1.0' consistent; no stray other versions in new docs.
  4. Test-count consistency in the 14 new + 4 modified files: stale counts
     (520/520, 518, 21/21, old docs_count 13 claims) must not appear;
     required canonical counts must appear where referenced.
  5. Referenced repo paths inside backticks in new docs exist (spot list).
Exit 0 = clean, 1 = findings.
"""
import os, re, sys

ROOT = "/home/user/sniper-suite"
NEW_DOCS = [
    "docs/BUYER-OVERVIEW.md", "docs/CAPABILITY-MATRIX.md",
    "docs/BUYER-DUE-DILIGENCE.md", "docs/IP-COMPONENTS.md",
    "docs/THIRD-PARTY.md", "docs/BUYER-DEPLOYMENT.md",
    "docs/ACCEPTANCE-CHECKLIST.md", "docs/RELEASE-NOTES-0.1.0.md",
    "docs/BUYER-FAQ.md", "docs/SCOPE-BOUNDARY.md",
    "docs/SUPPORT-HANDOVER.md", "docs/BUYER-RISK-REGISTER.md",
    "docs/TECHNICAL-DIFFERENTIATORS.md", "docs/DELIVERY-MANIFEST.md",
    "docs/FINAL-DELIVERY.md", "docs/BUYER-QUICKSTART.md",
    "docs/TECHNICAL-FACT-SHEET.md", "docs/SELLER-FACT-SHEET.md",
    "docs/SELLING-LISTING-SOURCE.md", "docs/DEMO-RUNBOOK.md",
    "docs/EVIDENCE-INDEX.md", "docs/REPOSITORY-MAP.md",
    "docs/ARCHIVE-CHECKLIST.md",
]
MODIFIED = ["README.md", "CHANGELOG.md", "docs/HANDOVER.md", "release-manifest.json"]
CHECK_FILES = NEW_DOCS + MODIFIED

findings = []

# 1. link check across ALL markdown in repo
link_re = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
md_files = []
for dirpath, dirnames, filenames in os.walk(ROOT):
    dirnames[:] = [d for d in dirnames if d not in (".git", "target")]
    for f in filenames:
        if f.endswith(".md"):
            md_files.append(os.path.join(dirpath, f))
for path in md_files:
    text = open(path, encoding="utf-8").read()
    for m in link_re.finditer(text):
        target = m.group(1)
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        target = target.split("#")[0]
        if not target:
            continue
        full = os.path.normpath(os.path.join(os.path.dirname(path), target))
        if not os.path.exists(full):
            findings.append(f"LINK {os.path.relpath(path, ROOT)}: missing target {target}")

# 2. invisible chars
bad_chars = {"\u200b": "ZWSP", "\u200e": "LRM", "\u200f": "RLM", "\u202a": "LRE",
             "\u202e": "RLO", "\ufeff": "BOM"}
for rel in [os.path.relpath(p, ROOT) for p in md_files]:
    text = open(os.path.join(ROOT, rel), encoding="utf-8").read()
    for ch, name in bad_chars.items():
        if ch in text:
            findings.append(f"CHAR {rel}: contains {name}")

# 3/4. counts + versions in new/modified docs
STALE = [r"520\s*/\s*520", r"518\s*/\s*518", r"21\s*/\s*21", r"19\s*/\s*1\b"]
CANON = {
    "docs/CAPABILITY-MATRIX.md": ["521", "23 / 23", "10 / 10", "4 / 4", "1 / 1", "48 / 48", "20 PASS"],
    "docs/RELEASE-NOTES-0.1.0.md": ["521 / 521", "20 PASS / 0 FAIL / 0 SKIP", "0.1.0", "9c677cd", "0e139c3"],
    "docs/BUYER-DUE-DILIGENCE.md": ["521/521", "146", "2,801,590", "0.1.0"],
    "docs/ACCEPTANCE-CHECKLIST.md": ["0e139c3", "9c677cd", "521/521"],
    "docs/FINAL-DELIVERY.md": ["0.1.0", "9c677cd", "0e139c3", "521 / 521", "20 PASS / 0 FAIL / 0 SKIP", "2,801,590"],
    "docs/TECHNICAL-FACT-SHEET.md": ["521", "48", "23 / 10 / 4 / 1", "1.98.1", "0.1.0"],
    "docs/EVIDENCE-INDEX.md": ["521/521", "23/23", "10/10", "4/4", "1/1", "48/48", "20 PASS", "2/2"],
    "docs/BUYER-QUICKSTART.md": ["0.1.0", "verify-delivery.sh", "release-check.sh"],
    "docs/DEMO-RUNBOOK.md": ["PREVIOUSLY VERIFIED", "VERIFIED"],
    "docs/ARCHIVE-CHECKLIST.md": ["9c677cd", "0e139c3", "EXCLUDE", "INCLUDE"],
    "docs/REPOSITORY-MAP.md": ["185", "49"],
}
for rel in CHECK_FILES:
    p = os.path.join(ROOT, rel)
    text = open(p, encoding="utf-8").read()
    # docs/HANDOVER.md legitimately mentions superseded counts as history
    # ("The earlier 518/518, db 21/21 figures predate ...") — exempt it.
    stale_pats = [] if rel == "docs/HANDOVER.md" else STALE
    for pat in stale_pats:
        for m in re.finditer(pat, text):
            findings.append(f"STALE {rel}: '{m.group(0)}' (stale test count)")
    for needle in CANON.get(rel, []):
        if needle not in text:
            findings.append(f"MISSING {rel}: canonical fact '{needle}' not found")
    if rel.endswith(".md") and "0.1.0" not in text and rel not in ("docs/DELIVERY-MANIFEST.md",):
        pass  # version mention not mandatory in every doc

# version drift: any x.y.z that is not 0.1.0 mentioned as the product version
for rel in CHECK_FILES:
    text = open(os.path.join(ROOT, rel), encoding="utf-8").read()
    for m in re.finditer(r"sniper-suite (\d+\.\d+\.\d+)", text):
        if m.group(1) != "0.1.0":
            findings.append(f"VERSION {rel}: 'sniper-suite {m.group(1)}'")

# manifest sanity
import json
man = json.load(open(os.path.join(ROOT, "release-manifest.json")))
if man["version"] != open(os.path.join(ROOT, "VERSION")).read().strip():
    findings.append("MANIFEST: version != VERSION file")
docs_actual = len([f for f in os.listdir(os.path.join(ROOT, "docs")) if f.endswith(".md")])
if man["components"]["docs_count"] != docs_actual:
    findings.append(f"MANIFEST: docs_count {man['components']['docs_count']} != actual {docs_actual}")
if man["test_counts"]["workspace_total"] != 537:
    findings.append("MANIFEST: workspace_total != 537 (audit pass)")

# 5. spot-check referenced source paths exist
paths_referenced = set()
path_re = re.compile(r"`((?:crates|programs|scripts|\.github)/[A-Za-z0-9_./{}\-]+)`")
for rel in CHECK_FILES:
    if not rel.endswith(".md"):
        continue
    text = open(os.path.join(ROOT, rel), encoding="utf-8").read()
    for m in path_re.finditer(text):
        paths_referenced.add(m.group(1))
for pr in sorted(paths_referenced):
    if "{" in pr:  # brace-expanded illustrative lists: check prefix dir
        prefix = pr.split("{")[0].rsplit("/", 1)[0]
        if not os.path.isdir(os.path.join(ROOT, prefix)):
            findings.append(f"PATH {pr}: brace-list dir missing")
        continue
    cand = pr.rstrip(".")
    # allow glob-ish suffixes like 0001`-`0011 ranges
    if not os.path.exists(os.path.join(ROOT, cand)):
        # try as directory-ish prefix (e.g. crates/core/migrations/0001)
        if not any(f.startswith(os.path.basename(cand)) for f in
                   os.listdir(os.path.join(ROOT, os.path.dirname(cand)) or ROOT)
                   if os.path.isdir(os.path.join(ROOT, os.path.dirname(cand)) or ROOT)):
            findings.append(f"PATH {pr}: does not exist")

# tracked file count claim check (frozen tree = 146; now 146 + 14 new docs = 160)
all_files = []
for dirpath, dirnames, filenames in os.walk(ROOT):
    dirnames[:] = [d for d in dirnames if d not in (".git", "target")]
    for f in filenames:
        all_files.append(os.path.relpath(os.path.join(dirpath, f), ROOT))
total = len(all_files)
if total != 185:
    findings.append(f"FILECOUNT: repo now has {total} files, expected 185 (183 handover-pass baseline + 2 forensic-cycle docs)")

print(f"checked {len(md_files)} markdown files, {len(paths_referenced)} referenced paths, {total} repo files")
if findings:
    print("FINDINGS:")
    for f in findings:
        print(" -", f)
    sys.exit(1)
print("DOC CONSISTENCY: CLEAN")
