/**
 * no-fake-data contract test (GAP MAP v2, Part 5).
 *
 * The control plane is allowed to show exactly two things: real data returned
 * by the API, or an honest empty/error state. This test enforces that by
 * statically scanning every file under `src/` (except test directories) for:
 *
 *   1. The same forbidden canned-data / unsupported-claim pattern set that
 *      `scripts/forbid-fake-data.sh` enforces in CI — and it re-reads that
 *      shell script and fails if the two pattern sets drift apart.
 *   2. Common fake-data literals (lorem, canned identities, "sample data").
 *   3. Catch blocks that "recover" by installing non-empty sample arrays or
 *      object literals into state — an error must surface as an error or an
 *      empty state (`setX([])`), never as invented rows.
 *
 * This test is intentionally dependency-free (Node fs only) so it runs under
 * `vitest run` without a DOM, and it fails the build loudly rather than
 * silently allowing a regression to ship.
 */
import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const APP_ROOT = resolve(HERE, "../.."); // apps/control-plane
const REPO_ROOT = resolve(APP_ROOT, "../..");
const SRC = join(APP_ROOT, "src");
const FORBID_SCRIPT = join(REPO_ROOT, "scripts", "forbid-fake-data.sh");

/** Directory names never scanned (test fixtures may contain examples). */
const EXCLUDED_DIRS = new Set(["__tests__", "tests", "node_modules", ".next"]);

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
 * The canonical canned-data pattern set. MUST stay byte-for-byte in sync with
 * the PATTERN variable in scripts/forbid-fake-data.sh (a dedicated test below
 * proves the sync; update both together).
 */
export const CI_FORBIDDEN_PATTERN =
  "act-01|alt-01|tkt-2026-9481|acme-quant|4,850,290|4_850_290|4,850,000|4_850_000|185\\.5|1,420,000|1_420_000|198\\.51\\.100|203\\.0\\.113|FIPS[[:space:]]+140|SOC[[:space:]]*2|SOC2|HSM|sub-millisecond|zero-latency|Connected[[:space:]]*&[[:space:]]*Verified";

/** Convert a POSIX ERE fragment to a JS RegExp source (only the classes we use). */
function posixToJs(source: string): string {
  return source
    .replace(/\[\[:space:\]\]/g, "\\s");
}

/** Extra fake-data literals this test owns (beyond the CI shell script set). */
const EXTRA_FORBIDDEN: Array<[string, RegExp]> = [
  ["lorem-ipsum filler text", /lorem\s+ipsum/i],
  ["canned person identity (John/Jane Doe)", /\b(john|jane)\s+doe\b/i],
  ["'sample data' marker", /sample\s+data/i],
  ["'dummy data' marker", /dummy\s+data/i],
  ["'fake data' marker", /fake\s+data/i],
  ["'placeholder data' marker", /placeholder\s+(data|rows?|records?)\b/i],
  ["hardcoded demo money constant", /\$\s?4,850,290|\b4850290\b/],
];

interface Finding {
  file: string;
  line: number;
  rule: string;
  text: string;
}

function lineNumberAt(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i += 1) {
    if (source[i] === "\n") line += 1;
  }
  return line;
}

/**
 * Extract the body (between braces, brace-balanced) of every `catch` clause
 * in the file. Deliberately conservative: strings/comments can confuse it, so
 * false negatives here are tolerated — the forbidden-literal scan above is the
 * hard gate. False POSITIVES are not tolerated, hence the narrow heuristics.
 */
function catchBodies(source: string): Array<{ body: string; start: number; param: string }> {
  const bodies: Array<{ body: string; start: number; param: string }> = [];
  const catchRe = /\bcatch\b\s*(?:\(\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^)]*)?\))?\s*\{/g;
  let match: RegExpExecArray | null;
  while ((match = catchRe.exec(source)) !== null) {
    const open = source.lastIndexOf("{", match.index + match[0].length);
    if (open === -1) break;
    let depth = 0;
    let close = -1;
    for (let i = open; i < source.length; i += 1) {
      if (source[i] === "{") depth += 1;
      else if (source[i] === "}") {
        depth -= 1;
        if (depth === 0) {
          close = i;
          break;
        }
      }
    }
    if (close === -1) break;
    bodies.push({ body: source.slice(open + 1, close), start: open, param: match[1] ?? "" });
    catchRe.lastIndex = close;
  }
  return bodies;
}

/**
 * A catch body "fabricates data" when it assigns a NON-empty array literal to
 * a React state setter (setSomething(...)). Honest recovery — `setX([])`,
 * `setX(null)`, `setError(messageFromTheCaughtError)`, rethrow, toast — is
 * allowed and must keep passing.
 *
 * Object literals (`setMsg({ text: err… })`) are deliberately NOT flagged:
 * the codebase's honest pattern is to build a message object from the caught
 * error, and distinguishing "message about the error" from "invented object"
 * statically produces false positives. Arrays of rows are the fabrication
 * vector this gate targets.
 */
function fabricatesData(catchBody: string, catchParam: string): RegExpMatchArray | null {
  // setX([{...}]) or setX([ {...} , ... ]) — non-empty object arrays
  const objectArray = /set[A-Z][A-Za-z0-9]*\(\s*\[\s*\{/;
  // setX([1, ...]) / setX(["a", ...]) — non-empty primitive arrays
  const primitiveArray = /set[A-Z][A-Za-z0-9]*\(\s*\[\s*("|\d)/;
  // setX({ prop: [{…}] }) — an object whose property VALUE is a non-empty
  // array of rows. This is the fabrication vector; the honest
  // `setMsg({ text: err… })` shape does not match because its value is not
  // an array literal.
  const objectWithArrayValue = /set[A-Z][A-Za-z0-9]*\(\s*\{\s*[A-Za-z_$][A-Za-z0-9_$]*\s*:\s*\[\s*(\{|"|\d)/;
  const hit =
    catchBody.match(objectArray) ??
    catchBody.match(primitiveArray) ??
    catchBody.match(objectWithArrayValue);
  if (!hit) return null;
  // If the fabricated-looking value is actually derived from the caught error
  // (`setX(err …)` style), it is honest reporting, not sample data.
  if (catchParam) {
    const window = catchBody.slice(hit.index ?? 0, (hit.index ?? 0) + 120);
    if (window.includes(catchParam)) return null;
  }
  return hit;
}

describe("no-fake-data contract", () => {
  const files = collectSourceFiles(SRC);

  it("has a non-empty source tree to scan", () => {
    expect(files.length).toBeGreaterThan(20);
  });

  it("CI pattern set is in sync with scripts/forbid-fake-data.sh", () => {
    const script = readFileSync(FORBID_SCRIPT, "utf8");
    const match = script.match(/^PATTERN='(.+)'$/m);
    expect(match, "PATTERN variable not found in forbid-fake-data.sh").toBeTruthy();
    expect(match![1]).toBe(CI_FORBIDDEN_PATTERN);
  });

  it("contains no forbidden canned-data or unsupported-claim literals", () => {
    const pattern = new RegExp(posixToJs(CI_FORBIDDEN_PATTERN), "i");
    const findings: Finding[] = [];
    for (const file of files) {
      const source = readFileSync(file, "utf8");
      let match: RegExpExecArray | null;
      const re = new RegExp(pattern.source, "gi");
      while ((match = re.exec(source)) !== null) {
        findings.push({
          file: relative(REPO_ROOT, file),
          line: lineNumberAt(source, match.index),
          rule: "ci-forbidden",
          text: match[0],
        });
      }
    }
    expect(findings, JSON.stringify(findings, null, 2)).toHaveLength(0);
  });

  it("contains no common fake-data markers", () => {
    const findings: Finding[] = [];
    for (const file of files) {
      const source = readFileSync(file, "utf8");
      for (const [rule, re] of EXTRA_FORBIDDEN) {
        let match: RegExpExecArray | null;
        const global = new RegExp(re.source, re.flags.includes("i") ? "gi" : "g");
        while ((match = global.exec(source)) !== null) {
          findings.push({
            file: relative(REPO_ROOT, file),
            line: lineNumberAt(source, match.index),
            rule,
            text: match[0],
          });
        }
      }
    }
    expect(findings, JSON.stringify(findings, null, 2)).toHaveLength(0);
  });

  it("never recovers from errors by installing sample data", () => {
    const findings: Finding[] = [];
    for (const file of files) {
      const source = readFileSync(file, "utf8");
      for (const { body, start, param } of catchBodies(source)) {
        const bad = fabricatesData(body, param);
        if (bad) {
          findings.push({
            file: relative(REPO_ROOT, file),
            line: lineNumberAt(source, start + (bad.index ?? 0)),
            rule: "catch-block-sample-data",
            text: bad[0].slice(0, 80),
          });
        }
      }
    }
    expect(findings, JSON.stringify(findings, null, 2)).toHaveLength(0);
  });
});
