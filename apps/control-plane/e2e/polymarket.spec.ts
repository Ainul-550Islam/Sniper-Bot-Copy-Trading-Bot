import { expect, test, type Page, type Route } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const now = "2026-10-30T00:00:00.000Z";
const orderId = "e2e-order-123456";
const orderRow = {
  id: orderId,
  market: "Will fixture event occur?",
  side: "BUY",
  status: "open",
  created_at: now,
};
const orderDetail = {
  venue_order_id: orderId,
  market: "Will fixture event occur?",
  side: "BUY",
  status: "open",
  price: 0.42,
  size: 12,
  created_at: now,
};

function polymarketStatus() {
  return {
    organization_id: organizationId,
    module: "polymarket",
    entitlement: { feature: "module.polymarket", granted: true },
    tenant_override: null,
    effective_state: "enabled",
    runtime: null,
    runtime_detail: "No runtime registered in this deployment",
    controls: { available: false, actions: [], scope: "tenant" },
  };
}

async function fillPolymarketForm(page: Page, name: string) {
  const values: Record<string, string> = {
    "Strategy Profile Name": name,
    "Market Condition IDs (one per line)": "condition-fixture-001",
    "Maximum Position (USDC)": "25",
    "Maximum Market Exposure (USDC)": "100",
    "Spread Threshold (BPS)": "120",
    "Reprice Interval (seconds)": "30",
  };
  for (const [label, value] of Object.entries(values)) {
    await page.getByLabel(label).fill(value);
  }
}

function successHandler(stored: Array<Record<string, unknown>>, createBodies: Array<Record<string, unknown>>) {
  return async (route: Route, pathname: string) => {
    if (pathname === "/api/tenant/polymarket/status") {
      await fulfillJson(route, 200, polymarketStatus());
      return;
    }
    if (pathname === "/api/tenant/polymarket/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/polymarket/orders" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [orderRow], next_cursor: null });
      return;
    }
    if (pathname === `/api/tenant/polymarket/orders/${orderId}` && route.request().method() === "GET") {
      await fulfillJson(route, 200, orderDetail);
      return;
    }
    if (pathname === "/api/tenant/polymarket/fills") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    if (pathname === "/api/tenant/polymarket/reconciliation") {
      await fulfillJson(route, 200, { organization_id: organizationId, drift: [] });
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
        id: "polymarket-profile-fixture",
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
  };
}

test("Polymarket profile saves, remains separate from runtime config, and links to authoritative order detail", async ({ page }) => {
  const stored: Array<Record<string, unknown>> = [];
  const createBodies: Array<Record<string, unknown>> = [];

  await signIn(page, successHandler(stored, createBodies));
  await goToViaNav(page, "Polymarket V3");
  await expect(page.getByText("No saved profiles.")).toBeVisible();
  await expect(page.getByText("No configuration recorded")).toBeVisible();

  await page.getByRole("button", { name: "Strategy Parameters ⚙" }).click();
  await fillPolymarketForm(page, "CLOB E2E Profile");
  await page.getByRole("button", { name: "Save Polymarket Strategy" }).click();
  await expect(page.getByRole("status")).toContainText("Polymarket strategy saved to the authenticated tenant.");

  expect(createBodies).toHaveLength(1);
  expect(createBodies[0]).toMatchObject({ name: "CLOB E2E Profile", module: "polymarket", mode: "paper" });
  expect(createBodies[0]?.config).toMatchObject({
    active_condition_ids: ["condition-fixture-001"],
    max_position_size_usdc_units: 25_000_000,
    max_market_exposure_usdc_units: 100_000_000,
    spread_threshold_bps: 120,
    dry_run: true,
  });

  await goToViaNav(page, "Polymarket V3");
  await expect(page.getByText("CLOB E2E Profile")).toBeVisible();
  await expect(page.getByText("No configuration recorded")).toBeVisible();
  await page.getByRole("link", { name: /e2e-order/ }).click();
  await expect(page).toHaveURL(new RegExp(`/trading/polymarket/orders/${orderId}$`));
  await expect(page.getByRole("heading", { name: `Polymarket Order: ${orderId.slice(0, 14)}…` })).toBeVisible();
  await expect(page.getByRole("region", { name: "Polymarket order detail" })).toContainText("Will fixture event occur?");
  await expect(page.getByRole("region", { name: "Polymarket order detail" })).toContainText("$0.4200");
});

test("Polymarket strategy form surfaces a server 4xx and keeps profile list empty", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/polymarket/status") {
      await fulfillJson(route, 200, polymarketStatus());
      return;
    }
    if (pathname === "/api/tenant/polymarket/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/polymarket/orders") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], next_cursor: null });
      return;
    }
    if (pathname === "/api/tenant/polymarket/fills") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    if (pathname === "/api/tenant/polymarket/reconciliation") {
      await fulfillJson(route, 200, { organization_id: organizationId, drift: [] });
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      await fulfillJson(route, 422, { error: "invalid_strategy_config", reason: "spread threshold is outside the venue bound" });
      return;
    }
    if (pathname === "/api/tenant/strategies") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Polymarket V3");
  await page.getByRole("button", { name: "Strategy Parameters ⚙" }).click();
  await fillPolymarketForm(page, "Rejected CLOB Profile");
  await page.getByRole("button", { name: "Save Polymarket Strategy" }).click();
  await expect(page.getByRole("status")).toContainText("spread threshold is outside the venue bound");
  await expect(page.getByRole("status")).not.toContainText("saved to the authenticated tenant");
});

test("Polymarket saved-profile inventory stays in loading state until the tenant response arrives", async ({ page }) => {
  let releaseProfiles: (() => void) | undefined;
  let signalProfilesRequest: (() => void) | undefined;
  const profilesRequested = new Promise<void>((resolve) => {
    signalProfilesRequest = resolve;
  });
  const fallback = successHandler([], []);

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/strategies" && route.request().method() === "GET") {
      signalProfilesRequest?.();
      await new Promise<void>((resolve) => {
        releaseProfiles = resolve;
      });
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fallback(route, pathname);
  });

  await goToViaNav(page, "Polymarket V3");
  await profilesRequested;
  await expect(page.getByText("Loading profiles…")).toBeVisible();
  expect(releaseProfiles).toBeDefined();
  releaseProfiles?.();
  await expect(page.getByText("No saved profiles.")).toBeVisible();
});
