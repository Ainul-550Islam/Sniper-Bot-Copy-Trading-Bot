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
# report); 2 = scanner could not run (missing inputs, or extraction
# failures beyond the tolerated 2% bound).
#
# Usage:
#   scripts/forensic-sql-scan.sh                 # sweep crates/** of this repo
#   scripts/forensic-sql-scan.sh <root>          # sweep another tree
#
# The second form is how tests/release/forensic_sql_extractor.sh runs the
# extractor against the fixtures in tests/release/forensic_sql_fixtures: any
# root without a `crates/` directory is swept whole (`**/*.rs`).
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
ROOT="$(cd "$ROOT" && pwd)"

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
    ("billing_webhook", "provider_events",
     "legacy billing webhook idempotency: (provider, provider_event_id) is unique at the provider and the organization is resolved from the verified payload before any state transition"),
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

# Statement-specific identity-plane sanctions, adjudicated by the
# integration-cleanup batch on 2026-10-28 and documented in
# docs/FORENSIC-SQL-RESEARCH-2026.md. Unlike SANCTIONED_GLOBAL above, an
# entry here is not a file/table exemption: it matches only when the exact
# normalized SQL fragment in the third element is present. Changing one of
# these statements therefore revokes its sanction and returns it to class 4
# until the replacement statement is reviewed. Every statement below runs
# before a trustworthy tenant context exists, resolves a globally unique
# identity or bearer credential, and is followed by tenant-scoped writes or
# canonical-user checks in the same flow.
IDENTITY_PLANE_SANCTIONS = [
    ("auth_flows", "invites", "SELECT id, organization_id, email, role, invited_by FROM invites WHERE token_hash = $1 AND status = 'pending' AND expires_at > now() FOR UPDATE",
     "public invitation acceptance: the SHA-256 token hash is the bearer credential and uniquely resolves the locked invitation; organization_id is read from that row rather than accepted from the caller, and every later write uses the locked organization"),
    ("auth_flows", "saas_runtime_records", "SELECT id, record FROM saas_runtime_records WHERE kind = 'user' AND lookup_key = $1 FOR UPDATE",
     "invited-account resolution: the email comes from the locked invitation row, user records are global identities keyed by the globally unique normalized email lookup key, and membership/session writes use the invitation's organization"),
    ("auth_flows", "users", "INSERT INTO users (id, email, email_verified, display_name, password_hash, status, platform_admin, created_at, updated_at, last_login_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (id) DO UPDATE SET email=EXCLUDED.email, email_verified=EXCLUDED.email_verified, display_name=EXCLUDED.display_name, password_hash=EXCLUDED.password_hash, status=EXCLUDED.status, platform_admin=EXCLUDED.platform_admin, updated_at=EXCLUDED.updated_at, last_login_at=EXCLUDED.last_login_at",
     "global identity projection during invitation acceptance: users has no organization_id column; the tenant relationship is written separately to organization_members with the locked invitation organization in the same transaction"),
    ("postgres", "users", "INSERT INTO users (id, email, email_verified, display_name, password_hash, status, platform_admin, created_at, updated_at, last_login_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (id) DO UPDATE SET email=EXCLUDED.email, email_verified=EXCLUDED.email_verified, display_name=EXCLUDED.display_name, password_hash=EXCLUDED.password_hash, status=EXCLUDED.status, platform_admin=EXCLUDED.platform_admin, updated_at=EXCLUDED.updated_at, last_login_at=EXCLUDED.last_login_at",
     "canonical global identity projection: users is keyed by globally unique user id and email and has no organization_id column; membership remains the tenant relationship and carries the organization predicate"),
    ("email_verification", "users", "SELECT id, email_verified, status FROM users WHERE lower(email) = lower($1)",
     "enumeration-safe verification request: a global account must be resolved by email before any tenant context exists; the endpoint returns the same accepted response for unknown addresses and binds any issued token to the resolved user id"),
    ("password_reset", "users", "SELECT id, status FROM users WHERE lower(email) = lower($1)",
     "enumeration-safe password-reset request: a global account must be resolved by email before any tenant context exists; the endpoint returns the same accepted response for unknown addresses and binds any issued token to the resolved user id"),
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
# Extraction: one record per sqlx::query* call site.
#
# The sweep is only as good as its extraction: a statement the scanner cannot
# read is a statement it never classifies, so the gate counts extraction
# failures and fails when more than EXTRACTION_FAILURE_BUDGET of the call sites
# in the tree could not be resolved to SQL text.
#
# What a call site's argument can be, in this codebase:
#   * an inline literal      — `sqlx::query("…")`, `sqlx::query(r#"…"#)`
#   * concatenated literals  — `sqlx::query("SELECT … " "FROM t")`
#   * a macro-built literal  — `sqlx::query(&format!("SELECT {COLS} FROM …"))`
#   * a branch of literals   — `sqlx::query(match x { Some(_) => "…", None => "…" })`
#   * a binding              — `const SQL: &str = r#"…"#;` / `let sql = if c { "…" } else { "…" };`
#   * the compile-time macros — `sqlx::query!("…")`, `query_as!`, `query_scalar!`
#   * a runtime variable     — `sqlx::query(statement)`: genuinely unresolvable
#     and therefore reported as an extraction failure.
# ---------------------------------------------------------------------------

EXTRACTION_FAILURE_BUDGET = 0.02  # 2% of call sites

# `sqlx::query`, `query_as`, `query_scalar`, the `!` macro forms, and the
# turbofish spellings (`sqlx::query_scalar::<_, bool>`). Group 1 is the opening
# paren, so the argument span can be found by bracket matching.
CALL_RE = re.compile(
    r'sqlx::query(?:_as|_scalar|scalar)?!?(?:::<[^<>]*>)?\s*(\()',
    re.DOTALL,
)

IDENT_RE = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')

SIMPLE_ESCAPES = {
    "n": "\n", "t": "\t", "r": "\r", "0": "\0",
    "\\": "\\", '"': '"', "'": "'",
}


def skip_ws_and_comments(text, i):
    n = len(text)
    while i < n:
        ch = text[i]
        if ch in " \t\r\n":
            i += 1
        elif text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j == -1 else j + 1
        elif text.startswith("/*", i):
            j = text.find("*/", i + 2)
            i = n if j == -1 else j + 2
        else:
            break
    return i


def starts_raw_string(text, i):
    """True when a raw string literal (`r"…"`, `r#"…"#`) starts at `i`."""
    if text[i] != "r":
        return False
    if i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_"):
        return False  # e.g. the `r` of `for"…"` is not a raw-string prefix
    j = i + 1
    while j < len(text) and text[j] == "#":
        j += 1
    return j < len(text) and text[j] == '"'


def read_string_literal(text, i):
    """Return (value, end_offset) for the Rust string literal at `i`, or None."""
    n = len(text)
    if i >= n:
        return None
    if starts_raw_string(text, i):
        j = i + 1
        while text[j] == "#":
            j += 1
        terminator = '"' + "#" * (j - i - 1)
        end = text.find(terminator, j + 1)
        if end == -1:
            return None
        return (text[j + 1:end], end + len(terminator))
    if text[i] != '"':
        return None
    out = []
    j = i + 1
    while j < n:
        ch = text[j]
        if ch == "\\" and j + 1 < n:
            out.append(SIMPLE_ESCAPES.get(text[j + 1], text[j + 1]))
            j += 2
            continue
        if ch == '"':
            return ("".join(out), j + 1)
        # A newline is legal inside a Rust string literal — most statements in
        # this codebase are written across several lines. Only an EOF without a
        # closing quote aborts (the `return None` below).
        out.append(ch)
        j += 1
    return None


def read_concatenated_literal(text, i):
    """Rust glues adjacent string literals: `"a" "b"` is one `&'static str`."""
    parts = []
    while True:
        j = skip_ws_and_comments(text, i)
        literal = read_string_literal(text, j)
        if literal is None:
            break
        parts.append(literal[0])
        i = literal[1]
    if not parts:
        return None
    return ("\n".join(parts), i)


def literal_at(text, i):
    j = skip_ws_and_comments(text, i)
    if text.startswith("&", j):
        j = skip_ws_and_comments(text, j + 1)
    return read_string_literal(text, j)


def bracket_span(text, i, opener="(", closer=")"):
    """Index of the `closer` matching the `opener` at `i`, or -1."""
    n = len(text)
    if i >= n or text[i] != opener:
        return -1
    depth = 0
    while i < n:
        ch = text[i]
        if ch == '"' or starts_raw_string(text, i):
            literal = read_string_literal(text, i)
            if literal is not None:
                i = literal[1]
                continue
        if ch == "'":
            j = i + 1
            while j < n and text[j] != "'":
                if text[j] == "\\":
                    j += 1
                j += 1
            i = j + 1
            continue
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j == -1 else j + 1
            continue
        if text.startswith("/*", i):
            j = text.find("*/", i + 2)
            i = n if j == -1 else j + 2
            continue
        if ch == opener:
            depth += 1
        elif ch == closer:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def collect_literals(text, start, end):
    """Every string literal in `text[start:end]`, in source order."""
    out = []
    i = start
    while i < end:
        literal = read_string_literal(text, i)
        if literal is not None:
            out.append(literal[0])
            i = literal[1]
            continue
        i += 1
    return out


def find_binding(text, name):
    """Span of the initializer of `const|static|let [mut] NAME = …;`."""
    pattern = (
        r"\b(?:const|static|let(?:\s+mut)?)\b\s+"
        + re.escape(name)
        + r"\b\s*(?::[^=;{]*)?=\s*"
    )
    for m in re.finditer(pattern, text):
        start = m.end()
        end = expression_end(text, start)
        if end != -1:
            return (start, end)
    return None


def expression_end(text, start):
    """End of the expression starting at `start` (the `;` closes it at depth 0)."""
    n = len(text)
    i = start
    depth = 0
    while i < n:
        ch = text[i]
        if ch == '"' or starts_raw_string(text, i):
            literal = read_string_literal(text, i)
            if literal is not None:
                i = literal[1]
                continue
        if ch == "'":
            j = i + 1
            while j < n and text[j] != "'":
                if text[j] == "\\":
                    j += 1
                j += 1
            i = j + 1
            continue
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j == -1 else j + 1
            continue
        if text.startswith("/*", i):
            j = text.find("*/", i + 2)
            i = n if j == -1 else j + 2
            continue
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == ";" and depth == 0:
            return i
        i += 1
    return -1


def extract_sql_at(text, i, depth=0):
    """
    Best-effort SQL text for the call argument starting at `i`.

    Returns (sql, end_offset). `sql` is None when the argument is built at
    runtime and cannot be resolved statically — the caller records that as an
    extraction failure so the gate can see the blind spot.
    """
    i = skip_ws_and_comments(text, i)
    if text.startswith("&", i):
        i = skip_ws_and_comments(text, i + 1)

    # (a) an inline literal, including Rust's adjacent-literal concatenation.
    literal = read_concatenated_literal(text, i)
    if literal is not None:
        return literal[0], literal[1]

    # (b) a macro-built literal: format!(…) / concat!(…). Interpolated
    #     placeholders ({}, {COLS}) stay in the text — they are reported by the
    #     G9 probe below and the statement is still classified.
    for macro in ("format!", "concat!"):
        if text.startswith(macro, i):
            paren = text.find("(", i + len(macro) - 1)
            if paren != -1:
                inner_end = bracket_span(text, paren)
                if inner_end != -1:
                    parts = collect_literals(text, paren + 1, inner_end)
                    if parts:
                        return "\n".join(parts), inner_end + 1

    # (c) `match … { Some(_) => "…", None => "…" }`: every arm is a statement
    #     that can ship, so every arm is classified.
    if re.match(r"match\b\s*[\(\s]", text[i:i + 8] + " "):
        brace = text.find("{", i)
        if brace != -1:
            brace_end = bracket_span(text, brace, "{", "}")
            if brace_end != -1:
                parts = collect_literals(text, i, brace_end)
                if parts:
                    return "\n".join(parts), brace_end + 1

    # (d) a binding in this file: `const SQL: &str = r#"…"#;` or
    #     `let sql = if c { "…" } else { "…" };`.
    if depth < 3:
        m = IDENT_RE.match(text, i)
        if m:
            bound = find_binding(text, m.group(0))
            if bound is not None:
                start, end = bound
                parts = collect_literals(text, start, end)
                if parts:
                    return "\n".join(parts), m.end()
                nested, _ = extract_sql_at(text, start, depth + 1)
                if nested:
                    return nested, m.end()

    return None, i


def in_line_comment(text, pos):
    line_start = text.rfind("\n", 0, pos) + 1
    return text[line_start:pos].lstrip().startswith("//")


def extract_statements(text, path, rel):
    """
    Yield (line, sql) for every sqlx::query* call site, and append
    (line, None) to `unresolved` for every site that could not be read.
    """
    out = []
    for m in CALL_RE.finditer(text):
        if in_line_comment(text, m.start()):
            continue  # documentation, not a call
        line = text.count("\n", 0, m.start()) + 1
        paren = m.start(1)
        arg_end = bracket_span(text, paren)
        arg_end = len(text) if arg_end == -1 else arg_end
        sql, _ = extract_sql_at(text, paren + 1)
        if sql is None:
            # Last resort: any literal anywhere in the argument list (an
            # if/else or a block expression the parser did not recognise).
            parts = collect_literals(text, paren + 1, arg_end)
            sql = "\n".join(parts) if parts else None
        if sql is None or not sql.strip():
            UNRESOLVED.append((line, rel))
            continue
        out.append((line, sql))
    return out


UNRESOLVED = []


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
    ins = re.search(r'(?i)\binsert\s+into\s+[a-z_][a-z0-9_]*[\s\\]*\(([^)]*)\)', sql)
    if ins and "organization_id" in ins.group(1).lower():
        return True
    arb = re.search(r'(?i)\bon\s+conflict\s*\(([^)]*)\)', sql)
    if arb and "organization_id" in arb.group(1).lower():
        return True
    return False


ORG_OK = re.compile(r'(?i)organization_id')  # projection-presence (for reports only)

findings = []   # dicts: group, cls, file, line, table, sql, rationale
per_file_statements = 0
call_sites = 0

if (root / "crates").is_dir():
    rs_files = sorted(set(list(root.glob("crates/*/src/**/*.rs")) + list(root.glob("crates/*/tests/*.rs"))))
else:
    # A fixture tree: every Rust file under the given root is in scope.
    rs_files = sorted(root.glob("**/*.rs"))
if not rs_files:
    print("[forensic-sql] no Rust sources found — refusing to report a clean sweep", file=sys.stderr)
    sys.exit(2)

for path in rs_files:
    text = path.read_text(errors="replace")
    rel = str(path.relative_to(root))
    # Test-file detection is relative to the swept root, so a fixture tree
    # under tests/release/… is judged by its own layout rather than by where it
    # happens to sit in the repository.
    is_test_file = "/tests/" in rel or rel.startswith("tests/")
    tspan = test_span(text)
    stmts = extract_statements(text, path, rel)
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
        normalized_sql = " ".join(sql.split())
        identity_sanctions = [
            s for s in IDENTITY_PLANE_SANCTIONS
            if s[0] == stem and s[1] in tables and s[2] in normalized_sql
        ]

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
        if identity_sanctions and not org_enforced(sql):
            for s in identity_sanctions:
                record("G10", 2, s[1], s[3])
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
    for m in re.finditer(r'(format!\s*\(\s*r?#\"?(?:\s*(?:SELECT|INSERT|UPDATE|DELETE)[^\"]*)\")', text):
        line = text.count("\n", 0, m.start()) + 1
        tspan_l = tspan
        cosmetic = is_test_file or (tspan_l is not None and line >= tspan_l)
        findings.append({
            "group": "G9", "class": 5 if cosmetic else 4, "file": rel, "line": line,
            "table": "(interpolated)", "sql": m.group(1)[:160],
            "why": "SQL assembled by format! — parameters must be bound, never interpolated",
        })

# Every call site in the tree, counted the same way it is extracted, so the
# coverage ratio below compares like with like.
for path in rs_files:
    text = path.read_text(errors="replace")
    call_sites += sum(
        1 for m in CALL_RE.finditer(text) if not in_line_comment(text, m.start())
    )

UNRESOLVED.sort(key=lambda item: (item[1], item[0]))
failure_ratio = (len(UNRESOLVED) / call_sites) if call_sites else 0.0

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
CLASS_NAME = {1: "tenant-safe", 2: "intentional-global", 3: "operator-only",
              4: "MISSING-TENANT-ENFORCEMENT", 5: "cosmetic"}

print("# FORENSIC SQL SWEEP (generated by scripts/forensic-sql-scan.sh)")
print()
print(f"Rust files swept: {len(rs_files)}   sqlx::query call sites: {call_sites}   statements extracted: {per_file_statements}")
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

# ---------------------------------------------------------------------------
# Extraction coverage: a statement the scanner cannot read is a statement it
# never classifies, so a blind spot above the budget fails the gate (exit 2)
# instead of being printed as a warning nobody reads.
# ---------------------------------------------------------------------------
print()
print("## Extraction coverage")
print(f"  call sites     : {call_sites}")
print(f"  extracted      : {per_file_statements}")
print(f"  unparsed       : {len(UNRESOLVED)} ({failure_ratio * 100:.2f}% — budget {EXTRACTION_FAILURE_BUDGET * 100:.0f}%)")
if UNRESOLVED:
    print("  unparsed statements (file:line) — SQL is built at runtime and escapes static analysis:")
    for line, rel in UNRESOLVED:
        print(f"    {rel}:{line}")
    print("[forensic-sql] EXTRACTION FAILURES:", f"{len(UNRESOLVED)}/{call_sites}", file=sys.stderr)

print()
if class4:
    print(f"[forensic-sql] FAIL — {len(class4)} class-4 (missing-tenant-enforcement) findings:")
    for f in class4:
        print(f"  {f['file']}:{f['line']} {f['table']} :: {f['sql']}")
    sys.exit(1)

if failure_ratio > EXTRACTION_FAILURE_BUDGET:
    print(
        f"[forensic-sql] FAIL — extraction coverage too low: {len(UNRESOLVED)}/{call_sites} "
        f"call sites unparsed ({failure_ratio * 100:.2f}% > {EXTRACTION_FAILURE_BUDGET * 100:.0f}% budget). "
        "The tenant-isolation contract is not proven for the statements listed above.",
        file=sys.stderr,
    )
    sys.exit(2)

print("[forensic-sql] PASS — zero class-4 findings; every tenant-table statement is org-scoped in SQL, sanctioned global, operator-only, or test-only")
sys.exit(0)
PY
