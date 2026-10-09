#!/usr/bin/env bash
# generate-stats.sh — ONE source of truth for repository numbers (P0-B TASK 3).
#
# Writes docs/STATS.md. Every value is wrapped as
#   <!-- stat:KEY -->VALUE<!-- /stat -->
# so tests/release/stats_current.sh can recompute and diff, and so buyer docs
# can embed the SAME markers instead of hand-typed numbers.
#
# Contract:
#   * offline and deterministic: no network, no timestamps, no cargo;
#     two runs back-to-back produce byte-identical output;
#   * test_attrs_* are STATIC COUNTS of test functions in the source tree.
#     They are NOT the result of a test run and must never be quoted as one;
#   * counting methods are stated in the table and mirrored EXACTLY by
#     tests/release/stats_current.sh (any method change must touch both).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/docs/STATS.md}"

python3 - "$ROOT" "$OUT" <<'PY'
import json, re, sys
from pathlib import Path

root = Path(sys.argv[1])
out = Path(sys.argv[2])

def count_lines(paths):
    n = 0
    for p in paths:
        n += p.read_text(errors="replace").count("\n") + 1
    return n

# --- crates: workspace members listed in the root Cargo.toml --------------
workspace_toml = (root / "Cargo.toml").read_text()
m = re.search(r"\[workspace\][\s\S]*?members\s*=\s*\[(.*?)\]", workspace_toml, re.S)
crates = len(re.findall(r'"([^"]+)"', m.group(1))) if m else 0

# --- programs: directories under programs/ containing a Cargo.toml --------
programs = sum(
    1 for d in sorted((root / "programs").iterdir())
    if d.is_dir() and (d / "Cargo.toml").exists()
) if (root / "programs").is_dir() else 0

# --- rust inventory under crates/ ------------------------------------------
rust_files = sorted((root / "crates").rglob("*.rs"))
rust_lines = count_lines(rust_files)

# literal-substring counts, same method as update-release-manifest.sh:
# file.read_text().count(needle) over every .rs under crates/
test_attrs_plain = sum(f.read_text(errors="replace").count("#[test]") for f in rust_files)
test_attrs_tokio = sum(f.read_text(errors="replace").count("#[tokio::test") for f in rust_files)

# --- staking inventory (own workspace, counted separately) -----------------
staking_rs = []
for sub in ("src", "tests"):
    d = root / "programs" / "staking-suite" / sub
    if d.is_dir():
        staking_rs += sorted(d.rglob("*.rs"))
rust_lines_staking = count_lines(staking_rs)
test_attrs_staking = sum(
    f.read_text(errors="replace").count(n)
    for f in staking_rs for n in ("#[test]", "#[tokio::test")
)

# --- migrations -------------------------------------------------------------
migrations = sorted((root / "crates" / "core" / "migrations").glob("*.sql"))
high_water = ""
if migrations:
    mm = re.match(r"(\d+)", migrations[-1].name)
    high_water = mm.group(1) if mm else ""

# --- docs: top-level docs/*.md EXCLUDING STATS.md (never count itself) -----
docs_canonical = sorted(
    p for p in (root / "docs").glob("*.md") if p.name != "STATS.md"
)

# --- control plane ----------------------------------------------------------
pages = sorted((root / "apps" / "control-plane" / "src" / "app").rglob("page.tsx"))
ts_files = sorted(p for p in (root / "apps" / "control-plane" / "src").rglob("*")
                  if p.suffix in (".ts", ".tsx"))
ts_lines = count_lines(ts_files)

# --- evidence status census ---------------------------------------------------
passed = not_run = other = 0
for f in sorted((root / "evidence").rglob("*.json")):
    try:
        status = str(json.loads(f.read_text()).get("status", "")).upper()
    except Exception:
        status = ""
    if status == "PASSED":
        passed += 1
    elif status == "NOT_RUN":
        not_run += 1
    else:
        other += 1

def marker(key, value):
    return f"<!-- stat:{key} -->{value}<!-- /stat -->"

# (key, value, counting method) — one row per key, method stated per row.
rows = [
    ("crates", crates,
     "workspace `members` listed in the root `Cargo.toml`"),
    ("programs", programs,
     "directories under `programs/` that contain a `Cargo.toml`"),
    ("rust_files", len(rust_files),
     "`find crates -type f -name '*.rs'`"),
    ("rust_lines", rust_lines,
     "total lines of those `.rs` files (newline count + 1 per file)"),
    ("rust_lines_staking", rust_lines_staking,
     "lines of `*.rs` under `programs/staking-suite/src` and `/tests`"),
    ("test_attrs_plain", test_attrs_plain,
     "occurrences of the literal `#[test]` under `crates/` "
     "(read_text().count, same method as `update-release-manifest.sh`)"),
    ("test_attrs_tokio", test_attrs_tokio,
     "occurrences of `#[tokio::test` under `crates/`"),
    ("test_attrs_staking", test_attrs_staking,
     "`#[test]` + `#[tokio::test` occurrences under `programs/staking-suite`"),
    ("migrations", len(migrations),
     "`crates/core/migrations/*.sql`"),
    ("migrations_high_water", high_water,
     "numeric prefix of the last migration file in sort order"),
    ("docs_canonical", len(docs_canonical),
     "top-level `docs/*.md` EXCLUDING `docs/STATS.md` (this file never counts itself)"),
    ("control_plane_pages", len(pages),
     "`find apps/control-plane/src/app -name page.tsx`"),
    ("control_plane_ts_lines", ts_lines,
     "lines of `*.ts` and `*.tsx` under `apps/control-plane/src`"),
    ("evidence_passed", passed,
     '`"status": "PASSED"` in `evidence/**/*.json`'),
    ("evidence_not_run", not_run,
     '`"status": "NOT_RUN"` in `evidence/**/*.json`'),
    ("evidence_other", other,
     "evidence JSON files whose status is neither PASSED nor NOT_RUN"),
]

lines = [
    "# Repository statistics — generated",
    "",
    "This file is GENERATED by `scripts/generate-stats.sh`. Do not hand-edit:",
    "`tests/release/stats_current.sh` regenerates it and fails the release on",
    "any drift, and README/docs embed these exact markers.",
    "",
    "**`test_attrs_*` are static counts of test functions in the source tree.",
    "They are NOT the result of a test run and must never be quoted as one.**",
    "",
    "| key | value | counting method |",
    "|---|---|---|",
]
for key, value, method in rows:
    lines.append(f"| `{key}` | {marker(key, value)} | {method} |")
lines.append("")

out.write_text("\n".join(lines))
try:
    shown = out.relative_to(root)
except ValueError:
    shown = out  # regenerating into a temp file (stats_current.sh)
print(f"[generate-stats] wrote {shown} ({len(rows)} keys)")
PY
