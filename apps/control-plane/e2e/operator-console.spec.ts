import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  fulfillJson,
  installApiMock,
  organizationId,
  sessionPayload,
  testOrganization,
  testUser,
} from "./helpers";

/**
 * The operator nav entry whose `href` is `path`.
 *
 * Locating by href rather than by accessible name is deliberate: the tenant
 * Settings group also carries an "API Keys" item (/settings/api), so a
 * name-based locator resolves to two links — and asserting on it would pass for
 * the wrong link. Matching the href is also what the D2 acceptance criterion
 * actually asks for: every OPERATOR_NAV href must resolve to a real page.
 */
function operatorNav(page: Page, path: string): Locator {
  return page
    .getByRole("navigation", { name: "Sections" })
    .locator(`a[href="${path}"]`);
}

/**
 * A key long enough for the backend's registry (>= 24 chars) and for the
 * unlock form. It is test material only: it unlocks nothing real because every
 * /api/** call in these specs is answered by `installApiMock`.
 */
const OPERATOR_KEY = "e2e-operator-key-material-0123456789";

test.describe("Operator Console", () => {
  test("platform admin unlocks with an operator key and every operator call carries x-api-key", async ({ page }) => {
    let moduleEnabled = true;
    /** Every Authorization / x-api-key header seen on /api/modules. */
    const moduleCalls: Array<{ apiKey: string | null; authorization: string | null; organization: string | null }> = [];

    await installApiMock(page, async (route, pathname) => {
      if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
        await fulfillJson(route, 200, {
          user: { ...testUser, platform_admin: true },
          token: "e2e-operator-token",
          session: sessionPayload(),
          organization_id: organizationId,
          organization_slug: testOrganization.slug,
        });
        return;
      }

      if (pathname === "/api/saas/users/me") {
        await fulfillJson(route, 200, {
          user: { ...testUser, platform_admin: true },
          organizations: [testOrganization],
        });
        return;
      }

      if (pathname === "/api/modules" && route.request().method() === "GET") {
        const headers = route.request().headers();
        moduleCalls.push({
          apiKey: headers["x-api-key"] ?? null,
          authorization: headers["authorization"] ?? null,
          organization: headers["x-organization"] ?? null,
        });
        await fulfillJson(route, 200, [
          {
            module: "sniper",
            enabled: moduleEnabled,
            running: true,
            healthy: true,
            connected: true,
            events_seen: 42,
            signals_generated: 12,
            orders_sent: 5,
            orders_filled: 5,
            orders_failed: 0,
            orders_rejected_by_risk: 0,
            realized_pnl: 1.25,
            unrealized_pnl: 0,
            consecutive_errors: 0,
            detail: "Raydium Sniper",
          },
        ]);
        return;
      }

      if (pathname === "/api/modules/sniper/disable" && route.request().method() === "POST") {
        moduleEnabled = false;
        await fulfillJson(route, 200, { ok: true, module: "sniper", enabled: false });
        return;
      }

      if (pathname === "/api/audit" && route.request().method() === "GET") {
        await fulfillJson(route, 200, {
          durable: true,
          count: 1,
          records: [
            {
              id: 1,
              ts: "2026-10-20T10:00:00Z",
              actor: "api:operator:ci",
              action: "module_disable",
              target: "sniper",
              outcome: "success",
              detail: {},
              hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
              chained: true,
            },
          ],
        });
        return;
      }

      if (pathname === "/api/audit/verify" && route.request().method() === "GET") {
        await fulfillJson(route, 200, { status: "intact", entries: 1 });
        return;
      }

      await fulfillJson(route, 200, {});
    });

    // 1. Sign in as a platform administrator.
    await page.goto("/");
    await page.getByLabel("Email").fill(testUser.email);
    await page.getByLabel("Password").fill("correct-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();

    // 2. Operator navigation group is visible (platform_admin only).
    const operatorGroup = page.getByRole("navigation", { name: "Sections" }).getByRole("heading", { name: "Operator" });
    await expect(operatorGroup).toBeVisible();

    // 3. The console is LOCKED until an operator key is presented.
    await page.getByRole("link", { name: "Modules" }).click();
    await expect(page.getByText("Operator Console Locked")).toBeVisible();
    await expect(page.getByLabel(/Operator API key/i)).toBeVisible();

    // 4. Unlock with the operator key.
    await page.getByLabel(/Operator API key/i).fill(OPERATOR_KEY);
    await page.getByRole("button", { name: /Unlock operator console/i }).click();

    // 5. Operator content renders and /api/modules carried the key — and
    //    neither the session token nor the tenant hint.
    await expect(page.getByRole("heading", { name: /Bot Modules & Supervisor/i })).toBeVisible();
    await expect(page.getByText("Raydium Sniper")).toBeVisible();

    await expect.poll(() => moduleCalls.length).toBeGreaterThan(0);
    for (const call of moduleCalls) {
      expect(call.apiKey).toBe(OPERATOR_KEY);
      expect(call.authorization).toBeNull();
      expect(call.organization).toBeNull();
    }

    // 6. Toggle module: click Disable and complete the typed confirmation.
    await page.getByRole("button", { name: "Disable" }).click();
    const confirmInput = page.getByLabel(/Type DISABLE SNIPER to confirm/i);
    await expect(confirmInput).toBeVisible();
    await confirmInput.fill("DISABLE SNIPER");
    await page.getByRole("button", { name: "Confirm disable" }).click();

    // 7. /operator/keys exists (Round 2, defect D2) and lists hashes only.
    // The locator targets the href, not the label: the tenant Settings group
    // also has an "API Keys" entry (/settings/api), so a name-based locator
    // matches two links and proves nothing about the operator route.
    await operatorNav(page, "/operator/keys").click();
    await expect(page.getByRole("heading", { name: /Operator API Keys/i })).toBeVisible();
    await expect(page.getByText(OPERATOR_KEY)).toHaveCount(0);

    // 8. Audit page still verifies the chain.
    await operatorNav(page, "/operator/audit").click();
    await expect(page.getByRole("heading", { name: /Immutable Audit Trail/i })).toBeVisible();
    await expect(page.getByText("module_disable")).toBeVisible();

    await page.getByRole("button", { name: /Verify Chain/i }).click();
    // Exact match: the "Cryptographic hash chain is intact across N records"
    // sentence contains the word too, so a substring locator is ambiguous.
    await expect(page.getByText("INTACT", { exact: true })).toBeVisible();
  });

  test("a 401 returns the console to the unlock prompt with the backend reason", async ({ page }) => {
    let modulesServed = 0;

    await installApiMock(page, async (route, pathname) => {
      if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
        await fulfillJson(route, 200, {
          user: { ...testUser, platform_admin: true },
          token: "e2e-operator-token",
          session: sessionPayload(),
          organization_id: organizationId,
          organization_slug: testOrganization.slug,
        });
        return;
      }

      if (pathname === "/api/saas/users/me") {
        await fulfillJson(route, 200, {
          user: { ...testUser, platform_admin: true },
          organizations: [testOrganization],
        });
        return;
      }

      if (pathname === "/api/modules" && route.request().method() === "GET") {
        modulesServed += 1;
        // First attempt: the backend refuses the key. After that the console is
        // locked again, so no second attempt is ever made.
        await fulfillJson(route, 401, {
          error: "unauthorized",
          reason: "invalid or missing x-api-key",
        });
        return;
      }

      await fulfillJson(route, 200, {});
    });

    await page.goto("/");
    await page.getByLabel("Email").fill(testUser.email);
    await page.getByLabel("Password").fill("correct-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();

    await page.getByRole("link", { name: "Modules" }).click();
    await page.getByLabel(/Operator API key/i).fill(OPERATOR_KEY);
    await page.getByRole("button", { name: /Unlock operator console/i }).click();

    // The refused key is dropped and the operator is back at the prompt with
    // the server's own words.
    await expect(page.getByText("Operator Console Locked")).toBeVisible();
    await expect(page.getByText("invalid or missing x-api-key")).toBeVisible();
    await expect(page.getByLabel(/Operator API key/i)).toHaveValue("");
    expect(modulesServed).toBe(1);
  });

  test("tenant owner who is not a platform admin cannot reach /operator content", async ({ page }) => {
    const ownerUser = { ...testUser, platform_admin: false };
    const ownerOrg = { ...testOrganization, role: "owner" };

    /**
     * Routes that only the operator console calls. If the shell ever mounted
     * operator content for this tenant owner, one of these would be requested —
     * and the backend would answer 401, because operator authority comes from
     * the operator key, not from a tenant role.
     */
    const OPERATOR_PATHS = [
      "/api/health",
      "/api/mode",
      "/api/modules",
      "/api/kill",
      "/api/resume",
      "/api/risk/kill-switch",
      "/api/keys",
      "/api/audit",
      "/api/journal",
      "/api/db",
      "/api/config",
      "/api/reconciliation/findings",
      "/api/recovery/failed",
      "/api/executions",
      "/api/trades",
      "/api/wallets",
      "/api/accounting/events",
    ];
    const operatorRequests: string[] = [];

    await installApiMock(page, async (route, pathname) => {
      if (
        OPERATOR_PATHS.some(
          (p) => pathname === p || pathname.startsWith(`${p}/`),
        )
      ) {
        operatorRequests.push(pathname);
      }

      if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
        await fulfillJson(route, 200, {
          user: ownerUser,
          token: "e2e-owner-token",
          session: sessionPayload(),
          organization_id: organizationId,
          organization_slug: ownerOrg.slug,
        });
        return;
      }

      if (pathname === "/api/saas/users/me") {
        await fulfillJson(route, 200, { user: ownerUser, organizations: [ownerOrg] });
        return;
      }

      await fulfillJson(route, 200, {});
    });

    await page.goto("/");
    await page.getByLabel("Email").fill(ownerUser.email);
    await page.getByLabel("Password").fill("correct-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();

    // The Operator nav group is NOT rendered for a tenant owner, so the app
    // offers no in-product path into the operator plane.
    const operatorGroup = page.getByRole("navigation", { name: "Sections" }).getByRole("heading", { name: "Operator" });
    await expect(operatorGroup).toHaveCount(0);
    await expect(operatorNav(page, "/operator/keys")).toHaveCount(0);

    // A hard load of an operator URL renders no operator surface at all. The
    // session lives in memory by design (never localStorage/sessionStorage), so
    // a fresh document at /operator has no session to authorise: the shell
    // mounts nothing rather than unlocking a console the backend would 401.
    //
    // The same denial WITH a live session — "Operator Access Required", no
    // unlock prompt, no operator request — is asserted in
    // src/__tests__/operator-guard.test.tsx, where the in-memory session can be
    // populated without a document load.
    await page.goto("/operator");
    await expect(page.getByLabel(/Operator API key/i)).toHaveCount(0);
    await expect(page.getByRole("heading", { name: /Bot Modules & Supervisor/i })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: /System Overview/i })).toHaveCount(0);

    await page.goto("/operator/keys");
    await expect(page.getByRole("heading", { name: /Operator API Keys/i })).toHaveCount(0);
    await expect(page.getByLabel(/Operator API key/i)).toHaveCount(0);

    // Not a single operator request was attempted on either hard load.
    expect(operatorRequests).toEqual([]);
  });
});
