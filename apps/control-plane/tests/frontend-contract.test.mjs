import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
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

/** Comments and doc-blocks describe rules; only executable code is asserted. */
function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
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

test("MFA login and invite enrollment remain limited until server promotion", () => {
  const api = read("apps/control-plane/src/lib/api.ts");
  const auth = read("apps/control-plane/src/lib/auth.ts");
  const loginPage = read("apps/control-plane/src/app/page.tsx");
  const invitePage = read("apps/control-plane/src/app/accept-invite/page.tsx");
  const enrollmentPage = read("apps/control-plane/src/app/mfa-enrollment/page.tsx");
  const e2e = read("apps/control-plane/e2e/mfa-authentication.spec.ts");

  assert.match(api, /mfa_code: mfaCode/);
  assert.match(auth, /if \(response\.mfa_enrollment_required\)/);
  assert.match(auth, /mfaEnrollmentRequired: response\.mfa_enrollment_required === true/);
  assert.match(loginPage, /router\.replace\("\/mfa-enrollment"\)/);
  assert.match(invitePage, /response\.mfa_enrollment_required/);
  assert.match(invitePage, /result\.session_promoted/);
  assert.match(enrollmentPage, /needsMfaEnrollment\(\)/);
  assert.match(enrollmentPage, /result\.session_promoted/);
  assert.match(e2e, /MFA login challenge/);
  assert.match(e2e, /only an enrollment session/);
  assert.match(e2e, /invitation acceptance enrolls, verifies/);
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

test("team members path is owned by the shared client and explicitly documented", () => {
  const client = read("apps/control-plane/src/lib/commercial.ts");
  const page = read("apps/control-plane/src/app/settings/team/page.tsx");
  const match = client.match(/export const TEAM_MEMBERS_PATH = "([^"]+)"/);
  assert.ok(match, "shared members path constant must exist");
  const spec = JSON.parse(read("openapi/openapi.json"));
  assert.ok(spec.paths[match[1]]?.get, "literal alias must have an OpenAPI GET contract");
  assert.match(client, /request<TeamListResponse>\(TEAM_MEMBERS_PATH\)/);
  assert.match(page, /await commercial\.members\(\)/);
  assert.match(page, /res\.members/);
  assert.doesNotMatch(page, /\/api\/saas\/organizations|\brequest\s*</);
});

test("next.config.ts uses buildStaticSecurityHeaders and operator-api exists", () => {
  const nextConfig = read("apps/control-plane/next.config.ts");
  assert.match(nextConfig, /buildStaticSecurityHeaders/);
  assert.match(
    nextConfig,
    /import\s*\{\s*buildStaticSecurityHeaders\s*\}\s*from\s*"\.\/src\/lib\/security-headers"/,
  );
  // The CSP is per-request (nonce) and therefore owned by src/proxy.ts, never
  // by the build-time headers() block.
  const executableConfig = stripComments(nextConfig);
  assert.doesNotMatch(executableConfig, /buildCsp/);
  assert.doesNotMatch(executableConfig, /"Content-Security-Policy"/);

  const operatorApi = read("apps/control-plane/src/lib/api/operator-api.ts");
  assert.match(operatorApi, /export\s+const\s+operatorApi\s*=/);
  assert.match(operatorApi, /export\s+async\s+function\s+getHealth\(/);
  assert.match(operatorApi, /export\s+async\s+function\s+killSystem\(/);
});

test("operator-api authenticates with x-api-key and never the tenant session", () => {
  const operatorApi = read("apps/control-plane/src/lib/api/operator-api.ts");

  // One private wrapper owns the credential; every operator function calls it.
  assert.match(operatorApi, /async\s+function\s+opRequest<T>\(/);
  assert.match(operatorApi, /"x-api-key":\s*key/);
  assert.match(operatorApi, /anonymous:\s*true/);
  assert.match(operatorApi, /class\s+OperatorKeyRequiredError\s+extends\s+Error/);
  assert.match(operatorApi, /error\.status\s*===\s*401/);
  assert.match(operatorApi, /clearOperatorKey\(error\.reason\)/);

  // `opRequest` is the only place allowed to call the shared client: going
  // direct would attach the tenant session token (Authorization) and the
  // x-organization hint, which the operator plane rejects with 401.
  const withoutWrapper = stripComments(operatorApi).replace(
    /async\s+function\s+opRequest<T>[\s\S]*?\n\}/,
    "",
  );
  assert.doesNotMatch(withoutWrapper, /request</);

  // Every exported operator function must go through opRequest.
  const exportedCalls =
    stripComments(operatorApi).match(
      /(?:^|\n)export\s+async\s+function\s+\w+\([\s\S]*?\n}\n/g,
    ) ?? [];
  assert.ok(exportedCalls.length >= 26, "operator function surface unexpectedly shrank");
  for (const fn of exportedCalls) {
    assert.match(fn, /opRequest</, `operator function must use opRequest: ${fn.slice(0, 60)}`);
  }
});

test("operator key is held in memory only and the /operator/keys page exists", () => {
  const session = read("apps/control-plane/src/lib/operator-session.ts");
  const executableSession = stripComments(session);

  // No persistence of any kind, and no credential in a URL or a log line.
  assert.doesNotMatch(
    executableSession,
    /\b(?:window\.)?(localStorage|sessionStorage|indexedDB|document\.cookie)\b/,
  );
  assert.doesNotMatch(executableSession, /console\.\w+/);
  assert.match(executableSession, /OPERATOR_KEY_IDLE_TTL_MS\s*=\s*15\s*\*\s*60\s*\*\s*1000/);
  assert.match(executableSession, /clearOperatorKey\(\)/);

  // The unlock prompt, the guard and the keys page must all exist.
  assert.match(
    read("apps/control-plane/src/components/operator/OperatorKeyPrompt.tsx"),
    /type="password"/,
  );
  assert.match(
    read("apps/control-plane/src/components/operator/OperatorGuard.tsx"),
    /OperatorKeyPrompt/,
  );
  const keysPage = read("apps/control-plane/src/app/operator/keys/page.tsx");
  assert.match(keysPage, /getKeys\(/);
  assert.match(keysPage, /<KeyList/);
  assert.match(keysPage, /protectedHash/);

  // Every Operator nav href resolves to a real page.
  const appShell = read("apps/control-plane/src/components/AppShell.tsx");
  const navBlock = /const OPERATOR_NAV: NavGroup = \{([\s\S]*?)\n\};/.exec(appShell);
  assert.ok(navBlock, "OPERATOR_NAV block must exist");
  const hrefs = [...navBlock[1].matchAll(/href:\s*"([^"]+)"/g)].map((m) => m[1]);
  assert.ok(hrefs.length >= 10, "operator nav unexpectedly shrank");
  for (const href of hrefs) {
    const file = `apps/control-plane/src/app${href === "/" ? "" : href}/page.tsx`;
    assert.ok(
      existsSync(join(ROOT, file)),
      `operator nav href ${href} has no page at ${file}`,
    );
  }
});

test("production CSP is nonce-based and never allows inline scripts", () => {
  const headers = read("apps/control-plane/src/lib/security-headers.ts");
  assert.match(headers, /export\s+function\s+buildCsp\(/);
  assert.match(headers, /export\s+function\s+buildStaticSecurityHeaders\(/);
  assert.match(headers, /'nonce-\$\{nonce\}'/);
  assert.match(headers, /'strict-dynamic'/);
  assert.doesNotMatch(headers, /scriptSrcList\.push\("\s*'unsafe-inline'"/);

  const proxy = read("apps/control-plane/src/proxy.ts");
  assert.match(proxy, /export\s+function\s+proxy\(/);
  assert.match(proxy, /crypto\.getRandomValues/);
  assert.match(proxy, /response\.headers\.set\(\s*\n?\s*"Content-Security-Policy"/);
  assert.match(proxy, /_next\/static/);
});
