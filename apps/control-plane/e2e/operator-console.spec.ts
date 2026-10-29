import { test, expect } from "@playwright/test";
import {
  fulfillJson,
  installApiMock,
  organizationId,
  sessionPayload,
  testOrganization,
  testUser,
} from "./helpers";

test.describe("Operator Console", () => {
  test("operator can view modules, toggle module state, and verify audit chain", async ({ page }) => {
    let moduleEnabled = true;

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
        await fulfillJson(route, 200, {
          status: "intact",
          entries: 1,
        });
        return;
      }

      await fulfillJson(route, 200, {});
    });

    // 1. Sign in as operator
    await page.goto("/");
    await page.getByLabel("Email").fill(testUser.email);
    await page.getByLabel("Password").fill("correct-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();

    // 2. Operator navigation group is visible
    const operatorGroup = page.getByRole("navigation", { name: "Sections" }).getByRole("heading", { name: "Operator" });
    await expect(operatorGroup).toBeVisible();

    // 3. Navigate to Modules page
    await page.getByRole("link", { name: "Modules" }).click();
    await expect(page.getByRole("heading", { name: /Bot Modules & Supervisor/i })).toBeVisible();
    await expect(page.getByText("Raydium Sniper")).toBeVisible();

    // 4. Toggle module: Click Disable and complete typed confirmation
    await page.getByRole("button", { name: "Disable" }).click();
    const confirmInput = page.getByLabel(/Type DISABLE SNIPER to confirm/i);
    await expect(confirmInput).toBeVisible();

    await confirmInput.fill("DISABLE SNIPER");
    await page.getByRole("button", { name: "Confirm disable" }).click();

    // 5. Navigate to Audit page and verify chain
    await page.getByRole("link", { name: "Audit Chain" }).click();
    await expect(page.getByRole("heading", { name: /Immutable Audit Trail/i })).toBeVisible();
    await expect(page.getByText("module_disable")).toBeVisible();

    await page.getByRole("button", { name: /Verify Chain/i }).click();
    await expect(page.getByText("INTACT")).toBeVisible();
  });

  test("tenant non-operator user cannot open /operator pages", async ({ page }) => {
    // Non-operator setup (viewer role, platform_admin false)
    const viewerUser = {
      ...testUser,
      platform_admin: false,
    };
    const viewerOrg = {
      ...testOrganization,
      role: "viewer",
    };

    await installApiMock(page, async (route, pathname) => {
      if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
        await fulfillJson(route, 200, {
          user: viewerUser,
          token: "e2e-viewer-token",
          session: sessionPayload(),
          organization_id: organizationId,
          organization_slug: viewerOrg.slug,
        });
        return;
      }

      if (pathname === "/api/saas/users/me") {
        await fulfillJson(route, 200, {
          user: viewerUser,
          organizations: [viewerOrg],
        });
        return;
      }

      await fulfillJson(route, 200, {});
    });

    // 1. Sign in as viewer
    await page.goto("/");
    await page.getByLabel("Email").fill(viewerUser.email);
    await page.getByLabel("Password").fill("correct-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();

    // 2. Operator navigation group is NOT rendered
    const operatorGroup = page.getByRole("navigation", { name: "Sections" }).getByRole("heading", { name: "Operator" });
    await expect(operatorGroup).toHaveCount(0);

    // 3. Attempt direct client navigation to /operator
    await page.goto("/operator");
    await expect(page.getByRole("heading", { name: "Operator Access Required" })).toBeVisible();
    await expect(page.getByText(/Platform administrator or organization operator permissions are required/i)).toBeVisible();
  });
});
