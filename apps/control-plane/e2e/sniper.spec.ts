import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const now = "2026-10-30T00:00:00.000Z";

function sniperStatus() {
  return {
    organization_id: organizationId,
    module: "sniper",
    entitlement: { feature: "module.sniper", granted: true },
    tenant_override: null,
    effective_state: "enabled",
    runtime: null,
    runtime_detail: "No runtime registered in this deployment",
    controls: { available: false, actions: [], scope: "tenant" },
  };
}

async function fillSniperForm(page: import("@playwright/test").Page, name: string) {
  const values: Record<string, string> = {
    "Strategy Profile Name": name,
    "Entry Amount (SOL)": "0.05",
    "Minimum Liquidity (SOL)": "1.25",
    "Maximum Slippage (BPS)": "250",
    "Take Profit (%)": "40",
    "Stop Loss (%)": "15",
    "Trailing Stop (%)": "10",
    "Auto-sell Timeout (seconds)": "300",
    "Priority Fee (lamports)": "5000",
  };
  for (const [label, value] of Object.entries(values)) {
    await page.getByLabel(label).fill(value);
  }
}

test("sniper profile saves to the tenant and appears in saved-profile status, not runtime config", async ({ page }) => {
  const stored: Array<Record<string, unknown>> = [];
  const createBodies: Array<Record<string, unknown>> = [];

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/sniper/status") {
      await fulfillJson(route, 200, sniperStatus());
      return;
    }
    if (pathname === "/api/tenant/sniper/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: stored, count: stored.length });
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      createBodies.push(body);
      const record = {
        id: "sniper-profile-fixture",
        organization_id: organizationId,
        name: body.name,
        description: body.description,
        module: body.module,
        mode: "paper",
        status: "active",
        version: 1,
        config_json: body.config,
        created_at: now,
        updated_at: now,
      };
      stored.push(record);
      await fulfillJson(route, 201, record);
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Sniper Bot");
  await expect(page.getByText("No saved profiles.")).toBeVisible();
  await page.getByRole("button", { name: "Strategy Parameters ⚙" }).click();
  await fillSniperForm(page, "Night Watch E2E");
  await page.getByRole("button", { name: "Save Sniper Configuration" }).click();
  await expect(page.getByRole("status")).toContainText("Sniper strategy saved to the authenticated tenant.");

  expect(createBodies).toHaveLength(1);
  expect(createBodies[0]).toMatchObject({ name: "Night Watch E2E", module: "sniper", mode: "paper" });
  expect(createBodies[0]?.config).toMatchObject({ entry_amount_lamports: 50_000_000, dry_run: true });

  await goToViaNav(page, "Sniper Bot");
  await expect(page.getByRole("heading", { name: "Saved strategy profiles" })).toBeVisible();
  await expect(page.getByText("Night Watch E2E")).toBeVisible();
  await expect(page.getByText(/do not by themselves change the active module configuration/)).toBeVisible();
  await expect(page.getByText("Runtime Phase:").locator(".." )).toContainText("idle");
});

test("sniper strategy form surfaces a server 4xx without showing a successful save", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/sniper/status") {
      await fulfillJson(route, 200, sniperStatus());
      return;
    }
    if (pathname === "/api/tenant/sniper/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      await fulfillJson(route, 422, { error: "invalid_strategy_config", reason: "sniper slippage exceeds the supported bound" });
      return;
    }
    if (pathname === "/api/tenant/strategies") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Sniper Bot");
  await page.getByRole("button", { name: "Strategy Parameters ⚙" }).click();
  await fillSniperForm(page, "Rejected sniper profile");
  await page.getByRole("button", { name: "Save Sniper Configuration" }).click();
  await expect(page.getByRole("status")).toContainText("sniper slippage exceeds the supported bound");
  await expect(page.getByRole("status")).not.toContainText("saved to the authenticated tenant");
});

test("sniper saved-profile inventory stays in loading state until the tenant response arrives", async ({ page }) => {
  let releaseProfiles: (() => void) | undefined;
  let signalProfilesRequest: (() => void) | undefined;
  const profilesRequested = new Promise<void>((resolve) => {
    signalProfilesRequest = resolve;
  });

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/sniper/status") {
      await fulfillJson(route, 200, sniperStatus());
      return;
    }
    if (pathname === "/api/tenant/sniper/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "GET") {
      signalProfilesRequest?.();
      await new Promise<void>((resolve) => {
        releaseProfiles = resolve;
      });
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Sniper Bot");
  await profilesRequested;
  await expect(page.getByText("Loading profiles…")).toBeVisible();
  expect(releaseProfiles).toBeDefined();
  releaseProfiles?.();
  await expect(page.getByText("No saved profiles.")).toBeVisible();
});
