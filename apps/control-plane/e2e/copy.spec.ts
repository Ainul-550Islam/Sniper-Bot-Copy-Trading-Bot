import { expect, test, type Page } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const now = "2026-10-30T00:00:00.000Z";

function copyStatus() {
  return {
    organization_id: organizationId,
    module: "copy",
    entitlement: { feature: "module.copy", granted: true },
    tenant_override: null,
    effective_state: "enabled",
    runtime: null,
    runtime_detail: "No runtime registered in this deployment",
    controls: { available: false, actions: [], scope: "tenant" },
  };
}

async function fillCopyForm(page: Page, name: string) {
  const values: Record<string, string> = {
    "Strategy Profile Name": name,
    "Maximum Exposure (USD)": "2500.00",
    "Allocation Per Trade (SOL)": "0.15",
    "Maximum Slippage (BPS)": "150",
    "Stale Event Timeout (seconds)": "45",
    "Copy Ratio (%)": "50",
  };
  for (const [label, value] of Object.entries(values)) {
    await page.getByLabel(label).fill(value);
  }
  await page.getByLabel("Leader Wallet Addresses (profile notes, one per line)").fill("7xE2EProfileNoteWallet");
}

test("copy profile saves as tenant strategy metadata while tracked leaders remain server-authoritative", async ({ page }) => {
  const stored: Array<Record<string, unknown>> = [];
  const createBodies: Array<Record<string, unknown>> = [];

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/copy/status") {
      await fulfillJson(route, 200, copyStatus());
      return;
    }
    if (pathname === "/api/tenant/copy/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/copy/leaders") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
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
        id: "copy-profile-fixture",
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

  await goToViaNav(page, "Copy Trading");
  await expect(page.getByText("No saved profiles.")).toBeVisible();
  await expect(page.getByText(/No leader wallets configured yet/)).toBeVisible();
  await page.getByRole("button", { name: "Copy Parameters ⚙" }).click();
  await fillCopyForm(page, "Copy E2E Profile");
  await page.getByRole("button", { name: "Save Copy Trading Configuration" }).click();
  await expect(page.getByRole("status")).toContainText("Copy trading strategy saved to the authenticated tenant.");

  expect(createBodies).toHaveLength(1);
  expect(createBodies[0]).toMatchObject({ name: "Copy E2E Profile", module: "copy", mode: "paper" });
  expect(createBodies[0]?.config).toMatchObject({ max_exposure_usd_cents: 250000, dry_run: true, lead_wallets: ["7xE2EProfileNoteWallet"] });

  await goToViaNav(page, "Copy Trading");
  await expect(page.getByText("Copy E2E Profile")).toBeVisible();
  await expect(page.getByText(/No leader wallets configured yet/)).toBeVisible();
});

test("copy strategy 4xx refusal remains visible and does not create a profile", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/copy/status") {
      await fulfillJson(route, 200, copyStatus());
      return;
    }
    if (pathname === "/api/tenant/copy/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/copy/leaders") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      await fulfillJson(route, 422, { error: "invalid_strategy_config", reason: "copy ratio exceeds the supported range" });
      return;
    }
    if (pathname === "/api/tenant/strategies") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Copy Trading");
  await page.getByRole("button", { name: "Copy Parameters ⚙" }).click();
  await fillCopyForm(page, "Rejected Copy Profile");
  await page.getByRole("button", { name: "Save Copy Trading Configuration" }).click();
  await expect(page.getByRole("status")).toContainText("copy ratio exceeds the supported range");
  await expect(page.getByRole("status")).not.toContainText("saved to the authenticated tenant");
});

test("copy saved-profile inventory stays in loading state until the tenant response arrives", async ({ page }) => {
  let releaseProfiles: (() => void) | undefined;
  let signalProfilesRequest: (() => void) | undefined;
  const profilesRequested = new Promise<void>((resolve) => {
    signalProfilesRequest = resolve;
  });

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/copy/status") {
      await fulfillJson(route, 200, copyStatus());
      return;
    }
    if (pathname === "/api/tenant/copy/config") {
      await fulfillJson(route, 200, null);
      return;
    }
    if (pathname === "/api/tenant/copy/leaders") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
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

  await goToViaNav(page, "Copy Trading");
  await profilesRequested;
  await expect(page.getByText("Loading profiles…")).toBeVisible();
  expect(releaseProfiles).toBeDefined();
  releaseProfiles?.();
  await expect(page.getByText("No saved profiles.")).toBeVisible();
});
