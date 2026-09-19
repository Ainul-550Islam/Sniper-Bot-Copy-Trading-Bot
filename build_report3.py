import os
ROOT = "/home/user/sniper-suite"
CREATED = ["docs/FINAL-DELIVERY.md","docs/BUYER-QUICKSTART.md","docs/TECHNICAL-FACT-SHEET.md",
"docs/SELLER-FACT-SHEET.md","docs/SELLING-LISTING-SOURCE.md","docs/DEMO-RUNBOOK.md",
"docs/EVIDENCE-INDEX.md","docs/REPOSITORY-MAP.md","docs/ARCHIVE-CHECKLIST.md",
"scripts/verify-delivery.sh"]
MODIFIED = ["docs/CAPABILITY-MATRIX.md","docs/BUYER-RISK-REGISTER.md","docs/IP-COMPONENTS.md",
"docs/DELIVERY-MANIFEST.md","README.md","docs/HANDOVER.md","CHANGELOG.md","release-manifest.json"]
out=[]; w=out.append
def dump(rel):
    p=os.path.join(ROOT,rel); data=open(p,encoding="utf-8").read()
    lang="bash" if rel.endswith(".sh") else ("json" if rel.endswith(".json") else "markdown")
    lines=data.count("\n")+(0 if data.endswith("\n") else 1)
    w(f"\n----- COMPLETE FILE: {rel} ({len(data.encode('utf-8'))} bytes, {lines} lines) -----\n")
    w("`````"+lang); w(data.rstrip("\n")); w("`````\n")

w("== SECTION 4: COMPLETE CONTENT OF EVERY CREATED FILE (10) ==\n")
for f in CREATED: dump(f)
w("\n== SECTION 5: COMPLETE CONTENT OF EVERY MODIFIED FILE (8) ==\n")
w("(CAPABILITY-MATRIX.md and BUYER-RISK-REGISTER.md were fully rewritten to the final column schema; the other six received targeted edits. All are reproduced complete below.)\n")
for f in MODIFIED: dump(f)
body="\n".join(out)
open("/home/user/_dumps.md","w",encoding="utf-8").write(body)
print("dumps:",len(body.encode()),"bytes; fences:",body.count("`````"))
