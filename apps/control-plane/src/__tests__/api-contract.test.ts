/**
 * api-contract test (GAP MAP v2, Part 5).
 *
 * Every `/api/...` endpoint the control plane actually calls must exist in
 * the committed OpenAPI contract (`openapi/openapi.json`). The reverse —
 * the contract documenting endpoints the UI never touches — is allowed; an
 * undocumented endpoint the UI DOES call is a drift bug that lets the
 * frontend and backend silently disagree.
 *
 * How it works:
 *   1. Walk every `.ts`/`.tsx` file under `src/` (except test dirs).
 *   2. Strip comments with a string-aware scanner so that prose like
 *      "we used to call `/api/foo`" is not mistaken for a call.
 *   3. Extract every string/template literal that starts with `/api/`.
 *   4. Normalize it: `${…}` interpolations become a `{p}` path segment;
 *      query strings are dropped; a trailing `{p}` that is directly
 *      concatenated to a word (a query-string template, not a path
 *      segment) is dropped.
 *   5. Match against the spec's `paths`, allowing a `{p}` segment to match
 *      a templated spec segment (and vice versa) so `/api/tenant/markets/123`
 *      matches `/api/tenant/markets/{id}`.
 *
 * A failure prints each undocumented path next to the file that calls it.
 */
import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const APP_ROOT = resolve(HERE, "../.."); // apps/control-plane
const REPO_ROOT = resolve(APP_ROOT, "../..");
const SRC = join(APP_ROOT, "src");
const SPEC_PATH = join(REPO_ROOT, "openapi", "openapi.json");

const EXCLUDED_DIRS = new Set(["__tests__", "tests", "node_modules", ".next"]);

/** String literals that are prefixes/building blocks, not endpoints. */
const PREFIX_CONSTANTS = new Set(["/api/tenant"]);

function collectSourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    if (EXCLUDED_DIRS.has(entry)) continue;
    const full = join(dir, entry);
    const stats = statSync(full);
    if (stats.isDirectory()) out.push(...collectSourceFiles(full));
    else if (/\.(ts|tsx)$/.test(entry)) out.push(full);
  }
  return out;
}

/**
 * Replace comments with spaces while preserving string/template literals
 * and newline offsets. String-aware so `//` inside a URL string survives.
 */
function stripComments(src: string): string {
  const out: string[] = [];
  let i = 0;
  const n = src.length;
  let quote: string | null = null; // ', ", or ` when inside a literal
  while (i < n) {
    const ch = src[i] ?? "";
    if (quote) {
      if (ch === "\\") {
        out.push(ch, src[i + 1] ?? "");
        i += 2;
        continue;
      }
      if (ch === quote) quote = null;
      out.push(ch);
      i += 1;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") {
      quote = ch;
      out.push(ch);
      i += 1;
      continue;
    }
    if (ch === "/" && src[i + 1] === "/") {
      while (i < n && src[i] !== "\n") {
        out.push(" ");
        i += 1;
      }
      continue;
    }
    if (ch === "/" && src[i + 1] === "*") {
      out.push("  ");
      i += 2;
      while (i < n && !(src[i] === "*" && src[i + 1] === "/")) {
        out.push(src[i] === "\n" ? "\n" : " ");
        i += 1;
      }
      if (i < n) {
        out.push("  ");
        i += 2;
      }
      continue;
    }
    out.push(ch);
    i += 1;
  }
  return out.join("");
}

/** Extract raw string/template literals starting with `/api/`. */
function extractApiLiterals(src: string): string[] {
  const result: string[] = [];
  const re = /(["'`])((?:\\.|(?!\1).)*)\1/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(src)) !== null) {
    const body = match[2] ?? "";
    if (body.startsWith("/api/")) result.push(body);
  }
  return result;
}

/**
 * Normalize a raw literal into a path template comparable to the spec:
 * `${…}` -> `{p}`, drop query string, drop a trailing concatenated `{p}`,
 * strip trailing slash.
 */
export function normalizeEndpoint(raw: string): string {
  let path = raw.replace(/\$\{[^}]*\}/g, "{p}");
  path = path.split("?")[0] ?? "";
  // A trailing `{p}` glued to a word char is a query-string template
  // (`/api/x${qs}`), not a path segment — drop it.
  if (path.endsWith("{p}") && !path.endsWith("/{p}")) {
    path = path.slice(0, -"{p}".length);
  }
  path = path.replace(/\/+$/, "");
  return path;
}

/** Segment-wise match where `{…}` on either side matches anything. */
function segmentsMatch(candidate: string, specPath: string): boolean {
  const a = candidate.split("/");
  const b = specPath.split("/");
  if (a.length !== b.length) return false;
  return a.every((seg, i) => {
    const other = b[i] ?? "";
    const aParam = seg.startsWith("{") && seg.endsWith("}");
    const bParam = other.startsWith("{") && other.endsWith("}");
    return seg === other || aParam || bParam;
  });
}

describe("api-contract", () => {
  const spec = JSON.parse(readFileSync(SPEC_PATH, "utf8")) as {
    info: { version: string };
    paths: Record<string, unknown>;
  };
  const specPaths = Object.keys(spec.paths);

  it("loads a non-trivial OpenAPI contract", () => {
    expect(specPaths.length).toBeGreaterThan(100);
    expect(spec.info.version).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("every /api endpoint used by the frontend is documented in openapi.json", () => {
    const files = collectSourceFiles(SRC);
    const missing: Array<{ file: string; endpoint: string }> = [];
    const seen = new Set<string>();

    for (const file of files) {
      const source = stripComments(readFileSync(file, "utf8"));
      for (const literal of extractApiLiterals(source)) {
        const endpoint = normalizeEndpoint(literal);
        if (!endpoint || PREFIX_CONSTANTS.has(endpoint)) continue;
        if (seen.has(endpoint)) continue;
        seen.add(endpoint);
        const documented = specPaths.some((sp) => segmentsMatch(endpoint, sp));
        if (!documented) {
          missing.push({ file: relative(REPO_ROOT, file), endpoint });
        }
      }
    }

    expect(missing, JSON.stringify(missing, null, 2)).toHaveLength(0);
  });

  it("normalizeEndpoint handles templates and query suffixes", () => {
    expect(normalizeEndpoint("/api/tenant/markets/${id}")).toBe("/api/tenant/markets/{p}");
    expect(normalizeEndpoint("/api/saas/alerts${qs}")).toBe("/api/saas/alerts");
    expect(normalizeEndpoint("/api/tenant/orders?limit=${limit}")).toBe("/api/tenant/orders");
    expect(normalizeEndpoint("/api/tenant/${module}/status")).toBe("/api/tenant/{p}/status");
    expect(normalizeEndpoint("/api/saas/team/members/${memberId}")).toBe(
      "/api/saas/team/members/{p}",
    );
  });
});
