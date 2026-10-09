/**
 * Strategy configuration e2e coverage (GAP MAP v2, Part 5).
 *
 * Exercises the tenant strategy console against mocked `/api/tenant/strategies`
 * responses: the honest empty state, the create round-trip (asserting the
 * exact Rust wire shape — `module`/`config`, `mode: "paper"`), the client-side
 * JSON validation refusal, and a server refusal surfaced verbatim. No fake
 * strategies are baked into the page; every row comes from a mocked response.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const createdRecord = {
  id: "7c1c0d3e-5a5a-4f6b-9c3a-2b6f8d0e1111",
  organization_id: organizationId,
  name: "E2E Launch Sniper",
  description: "Created by the e2e suite",
  module: "sniper",
  mode: "paper",
  status: "paused",
  version: 1,
  config_json: {
    target_tokens: ["So11111111111111111111111111111111111111112"],
    max_buy_sol: 1.5,
    slippage_bps: 100,
    anti_rug_min_liquidity_usd: 10000,
    auto_sell_take_profit_pct: 50,
    auto_sell_stop_loss_pct: 15,
    mev_protection_tip_lamports: 100000,
  },
  created_at: "2026-10-07T00:00:00Z",
  updated_at: "2026-10-07T00:00:00Z",
};

test("empty strategy list shows an honest empty state, then create posts the wire shape", async ({ page }) => {
  let stored: Array<Record<string, unknown>> = [];
  const createBodies: Array<Record<string, unknown>> = [];

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/strategies" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        items: stored,
        count: stored.length,
      });
      return;
    }
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      createBodies.push(body);
      stored = [{ ...createdRecord, name: body.name as string, description: body.description as string }];
      await fulfillJson(route, 201, stored[0]);
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Strategy Library");
  await expect(page.getByText("No Strategies Configured")).toBeVisible();

  await page.getByRole("button", { name: "Create Strategy" }).click();
  await expect(page.getByText("Create New Strategy")).toBeVisible();

  await page.getByPlaceholder("e.g. Raydium Launch Sniper").fill("E2E Launch Sniper");
  await page.getByPlaceholder("Execution parameters and anti-rug safeguards").fill("Created by the e2e suite");
  await page.getByRole("button", { name: "Save Strategy" }).click();

  await expect.poll(() => createBodies.length).toBe(1);
  const body = createBodies[0] as Record<string, unknown>;
  expect(body).toMatchObject({
    name: "E2E Launch Sniper",
    description: "Created by the e2e suite",
    module: "sniper",
    mode: "paper",
  });
  expect(body.config).toMatchObject({ slippage_bps: 100 });

  // After success the list reloads from the server and shows the new record.
  await expect(page.getByText("E2E Launch Sniper").first()).toBeVisible();
  await expect(page.getByText("No Strategies Configured")).toBeHidden();
});

test("invalid JSON parameters are refused client-side with no API call", async ({ page }) => {
  let createCalls = 0;
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/strategies") {
      if (route.request().method() === "POST") createCalls += 1;
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Strategy Library");
  await page.getByRole("button", { name: "Create Strategy" }).click();
  await page.getByPlaceholder("e.g. Raydium Launch Sniper").fill("Broken Strategy");
  await page.locator("textarea").fill("{not valid json");
  await page.getByRole("button", { name: "Save Strategy" }).click();

  await expect(page.getByText("Parameters must be valid JSON object")).toBeVisible();
  expect(createCalls).toBe(0);
});

test("a server refusal is surfaced verbatim and the empty state remains", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/strategies" && route.request().method() === "POST") {
      await fulfillJson(route, 422, {
        error: "invalid_strategy_config",
        reason: "sniper config rejected: slippage_bps must be between 1 and 5000",
      });
      return;
    }
    if (pathname === "/api/tenant/strategies") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Strategy Library");
  await page.getByRole("button", { name: "Create Strategy" }).click();
  await page.getByPlaceholder("e.g. Raydium Launch Sniper").fill("Rejected Strategy");
  await page.getByRole("button", { name: "Save Strategy" }).click();

  await expect(page.getByText(/sniper config rejected: slippage_bps must be between 1 and 5000/)).toBeVisible();
  await expect(page.getByText("No Strategies Configured")).toBeVisible();
});
