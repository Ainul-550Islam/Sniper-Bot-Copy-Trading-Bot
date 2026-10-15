#!/usr/bin/env python3
# sync-doc-stats.py — copy repository numbers from docs/STATS.md into every
# `<!-- stat:KEY -->VALUE<!-- /stat -->` marker in the docs.
#
# docs/STATS.md (written by scripts/generate-stats.sh) is the single source of
# truth. This script only rewrites the VALUE inside existing markers; it never
# invents a key, never edits text outside a marker, and fails when a marker
# names a key that STATS.md does not define. Run generate-stats.sh first, then
# this script, then tests/release/stats_current.sh.
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
STATS = ROOT / "docs" / "STATS.md"
MARK = re.compile(r"<!-- stat:([a-z_]+) -->(.*?)<!-- /stat -->", re.S)


def load_values() -> dict[str, str]:
    values = {m.group(1): m.group(2) for m in MARK.finditer(STATS.read_text(encoding="utf-8"))}
    if not values:
        sys.exit(f"sync-doc-stats: no stat markers found in {STATS}")
    return values


def main() -> int:
    values = load_values()
    changed = 0
    unknown: list[str] = []
    targets = [
        p
        for p in ROOT.rglob("*.md")
        if not any(part in {"target", "node_modules", ".git"} for part in p.parts)
        and p != STATS
    ]
    for path in sorted(targets):
        text = path.read_text(encoding="utf-8")
        if "<!-- stat:" not in text:
            continue

        def repl(m: re.Match[str]) -> str:
            nonlocal changed
            key, old = m.group(1), m.group(2)
            if key not in values:
                unknown.append(f"{path.relative_to(ROOT)}: {key}")
                return m.group(0)
            new = values[key]
            if old != new:
                changed += 1
            return f"<!-- stat:{key} -->{new}<!-- /stat -->"

        updated = MARK.sub(repl, text)
        if updated != text:
            path.write_text(updated, encoding="utf-8")
    if unknown:
        print("sync-doc-stats: markers with no STATS.md key:", file=sys.stderr)
        for item in unknown:
            print(f"  {item}", file=sys.stderr)
        return 1
    print(f"sync-doc-stats: updated {changed} marker value(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
