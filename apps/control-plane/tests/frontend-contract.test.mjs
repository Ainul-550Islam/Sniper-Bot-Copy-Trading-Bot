import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import assert from "node:assert/strict";

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(THIS_DIR, "../../..");
const APP = join(ROOT, "apps/control-plane");
const SRC = join(APP, "src");

function read(relativePath) {
  return readFileSync(join(ROOT, relativePath), "utf8");
}

function sourceFiles(directory) {
  const result = [];
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    const stats = statSync(path);
    if (stats.isDirectory()) result.push(...sourceFiles(path));
    else if (/\.(ts|tsx)$/.test(entry)) result.push(path);
  }
  return result;
}

test("frontend truthfulness gate rejects fake-data regressions", () => {
  let output = "";
  try {
    output = execFileSync("bash", [join(ROOT, "scripts/check-frontend-contract.sh")], {
      cwd: ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const detail = `${error.stdout ?? ""}${error.stderr ?? ""}`;
    assert.fail(`check-frontend-contract.sh failed:\n${detail}`);
  }
  assert.match(output, /check-frontend-contract: typecheck and truthfulness checks passed/);
});

test("customer trading client is tenant-only and fail-closed", () => {
  const client = read("apps/control-plane/src/lib/customer-trading-api.ts");

  assert.match(client, /const TENANT_PREFIX = "\/api\/tenant\/"/);
  assert.match(client, /if \(!path\.startsWith\(TENANT_PREFIX\)\)/);
  assert.match(client, /throw new Error\(/);

  const endpointLines = client
    .split("\n")
    .filter((line) => line.includes("tenantRequest"));
  assert.ok(endpointLines.length >= 20, "customer endpoint surface unexpectedly shrank");
  for (const line of endpointLines) {
    assert.doesNotMatch(line, /\/api\/(?:saas|ops|admin)\//);
  }
  assert.match(client, /customerTrading = \{/);
  assert.match(client, /\/api\/tenant\/orders/);
  assert.match(client, /\/api\/tenant\/positions/);
  assert.match(client, /\/api\/tenant\/strategies/);
});

test("shared API client keeps credentials out of URLs, cookies, and storage", () => {
  const api = read("apps/control-plane/src/lib/api.ts");

  assert.match(api, /headers\["authorization"\] = `Bearer \$\{token\}`/);
  assert.match(api, /headers\["x-organization"\] = tenant/);
  assert.match(api, /credentials: "omit"/);
  assert.match(api, /cache: "no-store"/);
  const executableApi = api
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\/\/.*$/gm, "");
  assert.doesNotMatch(executableApi, /\b(?:window\.)?(localStorage|sessionStorage|indexedDB)\b/);
  assert.doesNotMatch(executableApi, /\?token=|\?organization_id=/);
});

test("kill-switch contract remains confirmed, POST-only, and refreshes server truth", () => {
  const riskApi = read("apps/control-plane/src/lib/api/risk-api.ts");
  const panel = read("apps/control-plane/src/components/risk/KillSwitchPanel.tsx");

  assert.match(riskApi, /\/api\/saas\/risk-dashboard\/kill-switch/);
  assert.match(riskApi, /method: "POST"/);
  assert.match(riskApi, /body: input/);
  assert.match(panel, /if \(!confirm\(promptMsg\)\) return;/);
  assert.ok(panel.indexOf("confirm(promptMsg)") < panel.indexOf("toggleKillSwitch({"));
  assert.match(panel, /disabled=\{loading\}/);
  assert.match(panel, /onRefresh\(\)/);
  assert.match(panel, /finally \{/);
});

test("all frontend source files avoid obvious synthetic-data primitives", () => {
  const violations = [];
  for (const path of sourceFiles(SRC)) {
    const source = readFileSync(path, "utf8");
    if (/Math\.random\s*\(/.test(source)) violations.push(relative(ROOT, path));
    if (/fake(?:Data|_data)|sample(?:Data|_data)|mock(?:Data|_data)/i.test(source)) {
      violations.push(relative(ROOT, path));
    }
  }
  assert.deepEqual(violations, []);
});
