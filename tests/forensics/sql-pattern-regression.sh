#!/usr/bin/env bash
# tests/forensics/sql-pattern-regression.sh — regression gate for the
# forensic SQL sweep (PROMPT 6 §P5).
#
# Three assertions:
#   1. the real tree sweeps clean — ZERO class-4
#      (missing-tenant-enforcement) findings;
#   2. the scanner actually detects a planted unscoped tenant-table
#      statement (a scanner that cannot fail is worthless);
#   3. the scanner reports all five forensic classes in its summary
#      (the classification vocabulary stays intact).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SCAN="$ROOT/scripts/forensic-sql-scan.sh"

if [ ! -x "$SCAN" ]; then
  echo "[sql-regression] missing executable $SCAN" >&2
  exit 2
fi

echo "[sql-regression] 1/3 — the real tree must sweep with zero class-4 findings"
OUT="$("$SCAN")" || { printf '%s\n' "$OUT" | tail -30; echo "[sql-regression] FAIL — class-4 findings present (see above)" >&2; exit 1; }
echo "$OUT" | tail -1

echo "[sql-regression] 2/3 — the scanner must catch a planted unscoped statement"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# Scratch tree: link the real tree's crates/ entries, then add one file
# with a deliberately unscoped tenant-table statement. The scan derives
# its ROOT from its own location, so the script is copied inside too.
mkdir -p "$SCRATCH/scripts" "$SCRATCH/crates/core/src"
cp "$SCAN" "$SCRATCH/scripts/forensic-sql-scan.sh"
for entry in "$ROOT"/crates/*; do
  name="$(basename "$entry")"
  [ "$name" = "core" ] && continue
  ln -s "$entry" "$SCRATCH/crates/$name"
done
# core needs to be a real dir with src/: link everything except src, then
# plant src with our file plus a link to the real src's files.
for entry in "$ROOT"/crates/core/*; do
  name="$(basename "$entry")"
  [ "$name" = "src" ] && continue
  ln -s "$entry" "$SCRATCH/crates/core/$name"
done
for f in "$ROOT"/crates/core/src/*.rs; do
  name="$(basename "$f")"
  [ "$name" = "zz_planted_drift.rs" ] && continue
  ln -s "$(readlink -f "$f")" "$SCRATCH/crates/core/src/$name"
done
cat > "$SCRATCH/crates/core/src/zz_planted_drift.rs" <<'EOF'
// Planted by tests/forensics/sql-pattern-regression.sh — deliberately
// unscoped tenant-table access. The scanner MUST flag this as class 4.
pub async fn planted_unscoped(db: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT * FROM orders ORDER BY id DESC LIMIT 10")
        .fetch_all(db)
        .await?;
    Ok(())
}
EOF

if "$SCRATCH/scripts/forensic-sql-scan.sh" >/dev/null 2>&1; then
  echo "[sql-regression] FAIL — the scanner did NOT flag the planted unscoped statement" >&2
  exit 1
else
  echo "[sql-regression] planted unscoped statement detected — the scanner fails closed as required"
fi

echo "[sql-regression] 3/3 — the class vocabulary must stay intact"
for cls in "class 1 (tenant-safe)" "class 2 (intentional-global)" "class 3 (operator-only)" "class 4 (MISSING-TENANT-ENFORCEMENT)" "class 5 (cosmetic)"; do
  if ! printf '%s\n' "$OUT" | grep -qF "$cls"; then
    echo "[sql-regression] FAIL — class summary line missing: $cls" >&2
    exit 1
  fi
done
echo "[sql-regression] all five forensic classes reported"

echo "[sql-regression] PASS — zero class-4 on the real tree, drift detection proven, vocabulary intact"
exit 0
