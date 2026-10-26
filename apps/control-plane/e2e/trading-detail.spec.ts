/** Tenant trading detail e2e coverage (Part 04, L4).
 * Exercises the list→detail flows against the endpoints the pages actually
 * call: positions (GET /api/tenant/positions, then
 * GET /api/tenant/positions/{id} on the detail page), trades (GET
 * /api/tenant/trades list keyed by position, detail GET
 * /api/tenant/trades/{position_id}), balances (GET /api/tenant/balances with
 * the `{organization_id, items}` envelope) and recovery (GET
 * /api/tenant/recovery/intents + /api/tenant/recovery/sweep, both empty
 * states honest). No client-side synthesis anywhere.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const position = {
  id: "pos_1",
  symbol: "SOL/USDC",
  side: "long",
  qty: 10.5,
  entry_price: 142.5,
  mark_price: 145.0,
  realized_pnl: null,
  unrealized_pnl: 2.5,
  status: "open",
  opened_at: "2026-10-20T08:00:00Z",
  closed_at: null,
};

test("positions list links to the detail page, which GETs the server path", async ({ page }) => {
  let detailPath = "";

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/positions" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [position], next_cursor: null });
      return;
    }
    if (pathname === "/api/tenant/positions/pos_1" && route.request().method() === "GET") {
      detailPath = pathname;
      await fulfillJson(route, 200, position);
      return;
    }
    // The list page also mounts the Part-02 PnL card (GET /api/tenant/reports/pnl);
    // feed it its documented shape so the page under test renders.
    if (pathname === "/api/tenant/reports/pnl" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, period: "2026-10-26", pnl: 2.5 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Positions & PnL");
  await expect(page.getByRole("link", { name: "Details" }).first()).toBeVisible();
  await page.getByRole("link", { name: "Details" }).first().click();
  await expect(page).toHaveURL(/\/trading\/positions\/pos_1$/);
  await expect(page.getByText("SOL/USDC")).toBeVisible();
  expect(detailPath).toBe("/api/tenant/positions/pos_1");
});

test("trades list links by position id and the detail page GETs that path", async ({ page }) => {
  let detailPath = "";
  const trade = {
    id: "trd_1",
    position_id: "pos_1",
    symbol: "SOL/USDC",
    side: "buy",
    qty: 5,
    price: 140.0,
    venue: "raydium",
    status: "filled",
    executed_at: "2026-10-20T08:30:00Z",
  };

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/trades" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [trade], count: 1 });
      return;
    }
    if (pathname === "/api/tenant/trades/pos_1" && route.request().method() === "GET") {
      detailPath = pathname;
      await fulfillJson(route, 200, { position_id: "pos_1", trades: [trade] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Trades");
  // The list row shows the (truncated) trade id and links by position id.
  await expect(page.getByText(/^trd_1/)).toBeVisible();
  await page.getByRole("link", { name: /^pos_1/ }).click();
  await expect(page).toHaveURL(/\/trading\/trades\/pos_1$/);
  await expect(page.getByText("raydium")).toBeVisible();
  expect(detailPath).toBe("/api/tenant/trades/pos_1");
});

test("balances page GETs the balances path and shows server truth", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/balances" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        items: [
          { asset: "sol", symbol: "SOL", available: 10.5, locked: 2, value_usd_cents: 178750, updated_at: "2026-10-20T08:00:00Z" },
          { asset: "usdc", symbol: "USDC", available: 5000, locked: 0, value_usd_cents: 500000, updated_at: "2026-10-20T08:00:00Z" },
        ],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Balances");
  await expect(page.getByText("SOL", { exact: true })).toBeVisible();
  await expect(page.getByText("USDC", { exact: true })).toBeVisible();
  await expect(page.getByText("10.5")).toBeVisible();
});

test("recovery page GETs intents and sweeps and shows empty states honestly", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/recovery/intents" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    if (pathname === "/api/tenant/recovery/sweep" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Recovery");
  await expect(page.getByText("No recovery intents recorded.")).toBeVisible();
  await expect(page.getByText("No recovery sweeps recorded.")).toBeVisible();
});
