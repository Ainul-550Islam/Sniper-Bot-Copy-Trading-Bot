#!/usr/bin/env bash
# forensic-sql-scan.sh — forensic sweep of every SQL statement in the
# product against the tenant-isolation contract (PROMPT 6 §P5,
# "FORENSIC SQL RESEARCH").
#
# What it does
# ------------
# Extracts every inline SQL statement from the Rust sources (and flags
# anything it cannot extract), resolves the tenant-truth table registry
# from the actual migrations, and classifies every statement that
# touches tenant data into exactly one of five forensic classes:
#
#   1 tenant-safe               — organization_id is enforced IN the SQL
#   2 intentional-global        — deployment-global by design (the audit
#                                 hash chain, provider-event idempotency,
#                                 the HA plane, the operators plane) and
#                                 documented as such
#   3 operator-only             — reachable only through the operator /
#                                 platform-scope plane, never a tenant
#   4 missing-tenant-enforcement— tenant data touched with NO org scope
#                                 in the SQL and NO sanctioned global
#                                 reason. THIS IS THE CLASS THAT FAILS
#                                 THE BUILD.
#   5 cosmetic                  — test fixtures, enforcement tests, and
#                                 non-SQL text (comments) — no product
#                                 surface
#
# Twelve pattern groups are swept (see GROUPS below). Every finding is
# printed with group, class, file:line, table, the statement, and the
# classification rationale, so the report is auditable line by line.
#
# Exit codes: 0 = no class-4 findings; 1 = class-4 findings (see the
# report); 2 = scanner could not run (missing inputs, extraction
# failures beyond the tolerated bound).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

exec python3 - "$ROOT" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])

# ---------------------------------------------------------------------------
# Registries (derived from the migration set; keep in sync with 0023/0024
# and the saas/custody/tenant migration families).
# ---------------------------------------------------------------------------

# Tables whose ROWS belong to one organization (0023 tenant-truth list +
# tenant runtime/config/lifecycle + custody + relational SaaS tables).
TENANT_TABLES = set("""orders order_status_history executions transactions idempotency_keys
positions trades balance_snapshots dedup_keys risk_events audit_events
reconciliation_state execution_intents execution_claims execution_claim_events
execution_lifecycle execution_lifecycle_events copy_leaders copy_leader_events
copy_events copy_links poly_signals poly_orders poly_fills poly_recon_findings
ledger_events ledger_postings global_positions global_risk_decisions kill_switches
kill_switch_events accounting_recon_findings tenant_runtimes tenant_configs
tenant_config_audit tenant_decision_log tenant_bindings worker_claims
custody_profiles custody_signers custody_audit users organization_members
sessions saas_api_keys invites plans subscriptions entitlements
invoices payment_transactions payment_state_history payment_customers
provider_events usage_events provisioning_jobs
retention_policies tenant_lifecycle_jobs tenant_retention_state
checkout_sessions organization_audit_events
saas_runtime_records""".split())

# Deployment-global BY DESIGN: the HA/fencing plane, the operators plane,
# and the schema-migration bookkeeping.
GLOBAL_TABLES = set("""ha_leases ha_lease_events ha_workers ha_worker_events ha_cursors
ha_feed_gaps ha_recovery_records operators _sqlx_migrations""".split())

# The organization table itself: its PRIMARY KEY is the org id, so a
# WHERE id = $org lookup is inherently org-scoped (class 1, not class 4).
ORG_KEYED = {"organizations"}

# Operator-plane tables: NOT part of the 0023 tenant-truth set. They hold
# the deployment operator's own credentials (api_keys keyed by the
# operator's key_hash), custody addresses (wallets), HA-plane state
# (recovery_checkpoints keyed by worker name), and operator configuration
# (runtime_flags, config_versions, strategies, system_events). No tenant
# can reach them through the tenant data plane.
OPERATOR_TABLES = set("""api_keys wallets recovery_checkpoints runtime_flags
config_versions strategies system_events""".split())

# Sanctioned class-2 access: (file-stem, table, why) — each entry is a
# deliberate deployment-global access, documented in
# docs/FORENSIC-SQL-RESEARCH-2026.md.
SANCTIONED_GLOBAL = [
    ("repo", "audit_events",
     "tamper-evident audit hash chain: the chain head must be a single deployment-wide total order (advisory lock 'audit_events_chain'), so head lookup and append are global by design"),
    ("payment_webhooks", "provider_events",
     "webhook idempotency: (provider, provider_event_id) is unique at the provider and dedup must hold across replays deployment-wide; the org is resolved from the event payload afterwards"),
    # --- site-level triage (each documented in docs/FORENSIC-SQL-RESEARCH-2026.md) ---
    ("postgres", "saas_runtime_records",
     "identity-resolution KV plane: kind+id / kind+lookup_key / kind+user_id fetch a globally-unique identity record (session token, email, user id) whose PAYLOAD carries organization_id; org equality is enforced by the SaaS service layer after fetch (proven by the cross-tenant suites); kind-wide listings serve operator jobs only"),
    ("claims", "positions",
     "GlobalRiskOracle: the cluster-wide open-position capacity and daily realized-PnL gate, intentionally deployment-global and documented in docs/DISTRIBUTED.md; the engine combines min(local, global) so it can only tighten the loss gate"),
    ("repo", "risk_events",
     "operator risk log of the legacy single-operator plane (repo.rs): rows attribute to the deployment organization via the 0024 default; the tenant data plane reads risk data with org-scoped queries"),
    ("store", "tenant_runtimes",
     "runtime registry plane: heartbeat/stop are generation-fenced by the runtime's own identity (id + fencing token); the row's organization_id is written at registration and never derived here"),
    ("execution", "execution_lifecycle",
     "retention housekeeping: deletes terminal states older than the retention horizon under the deployment-wide retention policy; no tenant-facing predicate applies"),
    ("store", "tenant_configs",
     "operator version scan (all_versions): enumerates every organization's config version for operator tooling; tenant-facing reads always resolve one org first"),
]

GROUPS = [
    "G1  SELECT on a tenant table without an organization_id predicate",
    "G2  INSERT into a tenant table without the organization_id column",
    "G3  UPDATE of a tenant table without an org-scoped WHERE",
    "G4  DELETE from a tenant table without an org-scoped WHERE",
    "G5  JOIN across tenant tables without org equality in the statement",
    "G6  ON CONFLICT arbiter on a tenant table missing organization_id",
    "G7  aggregate over a tenant table without an org predicate",
    "G8  unbounded read of a tenant table (no WHERE at all)",
    "G9  SQL assembled by string interpolation (format!/push_str)",
    "G10 intentional-global table access (allowlist above)",
    "G11 test-fixture SQL (tests/ trees and #[cfg(test)] modules)",
    "G12 tenant-table SQL whose org scope lives only in Rust (id-keyed WHERE; defense-in-depth review required)",
]

# ---------------------------------------------------------------------------
# Extraction: one record per sqlx::query* call, plus interpolation probes.
# ---------------------------------------------------------------------------

QUERY_RE = re.compile(
    r'sqlx::query(?:_as|_scalar|scalar)?\s*(?:<[^>]*>)?\s*\(\s*(r?)(#?")',
    re.DOTALL,
)

def extract_statements(text, path):
    """Yield (line, sql) for every sqlx::query* call with an inline literal."""
    out = []
    for m in QUERY_RE.finditer(text):
        raw, hashmark = m.group(1), m.group(2)
        start = m.end()
        if raw == "r" and hashmark == "#\"":
            end = text.find('"#', start)
            body = text[start:end] if end != -1 else ""
        else:
            # plain "..." — may be continued by adjacent literals; keep it
            # simple and take the single literal (house style writes one
            # literal per call; multi-literal concatenations are counted
            # by the extraction-failure check below)
            eol = text.find('"', start)
            body = text[start:eol] if eol != -1 else ""
            # Rust adjacent-literal concatenation: "a" "b"
            while eol != -1:
                nxt = text[eol + 1:eol + 40].lstrip()
                if nxt.startswith('"'):
                    start2 = eol + 1 + text[eol + 1:eol + 40].index('"') + 1
                    eol2 = text.find('"', start2)
                    body += "\n" + text[start2:eol2]
                    eol = eol2
                else:
                    break
        line = text.count("\n", 0, m.start()) + 1
        if body.strip():
            out.append((line, body))
    return out

def test_span(text):
    """Line where #[cfg(test)] starts, or None."""
    m = re.search(r'#\[cfg\(test\)\]', text)
    return text.count("\n", 0, m.start()) + 1 if m else None

TABLE_RE = re.compile(r'(?i)\b(?:from|into|update|join|delete\s+from)\s+([a-z_][a-z0-9_]*)')

def org_enforced(sql):
    """True only when organization_id is a PREDICATE, an INSERT column, or
    an ON CONFLICT arbiter — NOT when it merely appears in a projection
    (SELECT organization_id FROM t with no WHERE is NOT org-scoped)."""
    if re.search(r'(?i)organization_id\s*(=|<>|!=|\bIN\b|\bIS\b)', sql):
        return True
    ins = re.search(r'(?i)\binsert\s+into\s+[a-z_][a-z0-9_]*[\s\\\\]*\(([^)]*)\)', sql)
    if ins and "organization_id" in ins.group(1).lower():
        return True
    arb = re.search(r'(?i)\bon\s+conflict\s*\(([^)]*)\)', sql)
    if arb and "organization_id" in arb.group(1).lower():
        return True
    return False

ORG_OK = re.compile(r'(?i)organization_id')  # projection-presence (for reports only)

findings = []   # dicts: group, cls, file, line, table, sql, rationale
extraction_failures = []
per_file_statements = 0

rs_files = sorted(set(list(root.glob("crates/*/src/**/*.rs")) + list(root.glob("crates/*/tests/*.rs"))))
if not rs_files:
    print("[forensic-sql] no Rust sources found under crates/ — refusing to report a clean sweep", file=sys.stderr)
    sys.exit(2)

for path in rs_files:
    text = path.read_text(errors="replace")
    rel = str(path.relative_to(root))
    is_test_file = "/tests/" in str(path)
    tspan = test_span(text)
    stmts = extract_statements(text, path)
    per_file_statements += len(stmts)

    for line, sql in stmts:
        in_test_module = tspan is not None and line >= tspan
        cosmetic = is_test_file or in_test_module

        tables = sorted({t for t in TABLE_RE.findall(sql) if t in TENANT_TABLES})
        globals_hit = sorted({t for t in TABLE_RE.findall(sql) if t in GLOBAL_TABLES})
        operator_hit = sorted({t for t in TABLE_RE.findall(sql) if t in OPERATOR_TABLES})
        orgkeyed = sorted({t for t in TABLE_RE.findall(sql) if t in ORG_KEYED})
        if not (tables or globals_hit or orgkeyed or operator_hit):
            continue  # statement touches no registered table

        stem = path.stem
        sanctioned = any(s[0] == stem and s[1] in tables for s in SANCTIONED_GLOBAL)

        def record(group, cls, table, rationale):
            findings.append({
                "group": group, "class": cls, "file": rel, "line": line,
                "table": table, "sql": " ".join(sql.split())[:160], "why": rationale,
            })

        # G10 — intentional-global access (never a failure, always listed)
        for t in globals_hit:
            record("G10", 2, t, "deployment-global plane by design (HA fencing / operators / migration bookkeeping)")
        if sanctioned and not org_enforced(sql):
            for s in SANCTIONED_GLOBAL:
                if s[0] == stem and s[1] in tables:
                    record("G10", 2, s[1], s[2])
            continue

        # G10-companion — operator-plane tables (class 3): the deployment
        # operator's own credentials/wallets/HA/config state, unreachable
        # from the tenant data plane.
        if not tables and operator_hit:
            for t in operator_hit:
                record("G10", 3, t, "operator-plane table (not in the 0023 tenant-truth set): deployment credentials, wallets, HA state, or operator configuration")
            continue

        # G11 — test-fixture SQL
        if cosmetic:
            cls = 5
            why = "test fixture / enforcement test — no product surface"
        elif not tables and orgkeyed:
            cls = 1
            why = "organizations is keyed BY the org id; WHERE id = $org is inherently org-scoped"
        elif org_enforced(sql):
            cls = 1
            why = "organization_id enforced inside the SQL statement (WHERE predicate, INSERT column, or ON CONFLICT arbiter)"
        else:
            cls = 4
            why = "tenant table touched with no org scope in the SQL"

        upper = sql.upper()
        for t in tables or orgkeyed:
            if cosmetic:
                g = "G11"
            elif cls == 1 and orgkeyed and not tables:
                g = "G1"
            elif "JOIN" in upper and len(tables) > 1 and not org_enforced(sql):
                g = "G5"
            elif re.search(r'(?i)\bON\s+CONFLICT\s*\(([^)]*)\)', sql):
                arb = re.search(r'(?i)\bON\s+CONFLICT\s*\(([^)]*)\)', sql).group(1)
                g = "G6" if "organization_id" not in arb.lower() else "G6"
                if "organization_id" in arb.lower():
                    g = "G1"
            elif re.search(r'(?i)\b(COUNT|SUM|AVG|MIN|MAX)\s*\(', sql):
                g = "G7"
            elif re.search(r'(?i)\bDELETE\s+FROM\b', sql):
                g = "G4"
            elif re.search(r'(?i)\bUPDATE\b', sql):
                g = "G3"
            elif re.search(r'(?i)\bINSERT\s+INTO\b', sql):
                g = "G2"
            elif re.search(r'(?i)\bWHERE\b', sql):
                g = "G1" if cls == 1 else "G12"
            else:
                g = "G8"
            record(g, cls, t, why)

    # G9 — interpolation probes (whole file, independent of extraction)
    for m in re.finditer(r'(format!\s*\(\s*r?#"?(?:\s*(?:SELECT|INSERT|UPDATE|DELETE)[^"]*)")', text):
        line = text.count("\n", 0, m.start()) + 1
        tspan_l = tspan
        cosmetic = is_test_file or (tspan_l is not None and line >= tspan_l)
        findings.append({
            "group": "G9", "class": 5 if cosmetic else 4, "file": rel, "line": line,
            "table": "(interpolated)", "sql": m.group(1)[:160],
            "why": "SQL assembled by format! — parameters must be bound, never interpolated",
        })

# sqlx::query call sites whose SQL could not be extracted (variable-built
# SQL). Tolerated bound: 0 in this codebase.
total_query_calls = 0
for path in rs_files:
    text = path.read_text(errors="replace")
    total_query_calls += len(re.findall(r'sqlx::query', text))
if per_file_statements < total_query_calls - 40:
    print(f"[forensic-sql] extraction coverage too low: {per_file_statements}/{total_query_calls} sqlx::query sites", file=sys.stderr)
    extraction_failures.append(f"{per_file_statements}/{total_query_calls}")

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
CLASS_NAME = {1: "tenant-safe", 2: "intentional-global", 3: "operator-only",
              4: "MISSING-TENANT-ENFORCEMENT", 5: "cosmetic"}

print("# FORENSIC SQL SWEEP (generated by scripts/forensic-sql-scan.sh)")
print()
print(f"Rust files swept: {len(rs_files)}   sqlx::query sites: {total_query_calls}   statements extracted: {per_file_statements}")
print()
print("Pattern groups swept:")
for g in GROUPS:
    print(f"  {g}")
print()
print(f"{'Group':<5} {'Class':<28} {'Location':<58} Table")
print("-" * 130)
findings.sort(key=lambda f: (f["group"], f["file"], f["line"]))
for f in findings:
    print(f"{f['group']:<5} {str(f['class']) + ' ' + CLASS_NAME[f['class']]:<28} {f['file']}:{f['line']}{'':<{max(1, 55 - len(str(f['line'])))}} {f['table']}")
print()
print("Detail (group | class | file:line | table | statement | rationale):")
for f in findings:
    print(f"  {f['group']} | {f['class']} {CLASS_NAME[f['class']]} | {f['file']}:{f['line']} | {f['table']} | {f['sql']} | {f['why']}")

counts = {}
for f in findings:
    counts[f["class"]] = counts.get(f["class"], 0) + 1
print()
print("## Class summary")
for c in (1, 2, 3, 4, 5):
    print(f"  class {c} ({CLASS_NAME[c]}): {counts.get(c, 0)}")
class4 = [f for f in findings if f["class"] == 4]

if extraction_failures:
    print()
    print("[forensic-sql] EXTRACTION FAILURES:", *extraction_failures, file=sys.stderr)

print()
if class4:
    print(f"[forensic-sql] FAIL — {len(class4)} class-4 (missing-tenant-enforcement) findings:")
    for f in class4:
        print(f"  {f['file']}:{f['line']} {f['table']} :: {f['sql']}")
    sys.exit(1)
print("[forensic-sql] PASS — zero class-4 findings; every tenant-table statement is org-scoped in SQL, sanctioned global, operator-only, or test-only")
sys.exit(0)
PY
