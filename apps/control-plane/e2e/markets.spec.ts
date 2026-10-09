/**
 * Markets screener e2e coverage (GAP MAP v2, Part 5 — markets page wiring).
 *
 * Locks the frontend↔data-plane contract: the screener renders rows from the
 * wire shape (integer cents/basis points converted for display), the venue
 * filter is derived from the feed itself, a degraded feed is disclosed, and a
 * 503 `market_data_unavailable` renders the refusal instead of fake tickers.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, signIn } from "./helpers";

const tickers = [
  {
    id: "pump_token_1",
    symbol: "BONK/SOL",
    name: "Bonk Launch Pair",
    venue: "pump_fun",
    base_asset: "BONK",
    quote_asset: "SOL",
    price_usd_cents: 42,
    change_24h_bps: 1234,
    volume_24h_usd_cents: 150_000_00,
    liquidity_usd_cents: 40_000_00,
    is_active: true,
    compatible_modules: ["sniper"],
    updated_at: "2026-10-07T00:00:00Z",
  },
  {
    id: "poly_market_1",
    symbol: "ELECTION-2028",
    name: "Next Presidential Winner",
    venue: "polymarket_clob",
    base_asset: "YES",
    quote_asset: "USDC",
    price_usd_cents: 53,
    change_24h_bps: -250,
    volume_24h_usd_cents: 9_000_000_00,
    liquidity_usd_cents: 2_500_000_00,
    is_active: true,
    compatible_modules: ["polymarket"],
    updated_at: "2026-10-07T00:00:00Z",
  },
];

test("screener renders wire-format tickers with unit conversion and feed-derived venues", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/markets" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        items: tickers,
        count: tickers.length,
        fetched_at: "2026-10-07T00:00:01Z",
        from_cache: false,
        feeds: [
          { name: "gamma", ok: true, detail: "", fetched_at: "2026-10-07T00:00:01Z", tickers: 1 },
          { name: "jupiter", ok: true, detail: "", fetched_at: "2026-10-07T00:00:01Z", tickers: 1 },
        ],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Market Screener");

  // Cent→dollar and bps→percent conversions are visible in the rows.
  await expect(page.getByText("BONK/SOL").first()).toBeVisible();
  await expect(page.getByText("$0.42").first()).toBeVisible();
  await expect(page.getByText("+12.34%").first()).toBeVisible();
  await expect(page.getByText("$150,000.00").first()).toBeVisible();
  await expect(page.getByText("ELECTION-2028").first()).toBeVisible();
  await expect(page.getByText("-2.50%").first()).toBeVisible();

  // Venue labels come from the wire enum, and the filter only lists venues
  // the feed actually reported.
  await expect(page.locator("tbody").getByText("Pump.fun").first()).toBeVisible();
  await expect(page.locator("tbody").getByText("Polymarket CLOB").first()).toBeVisible();
  const venueOptions = page.locator("select option");
  await expect(venueOptions).toHaveCount(3); // All Venues + pump_fun + polymarket_clob
  await expect(page.locator('select option[value="pump_fun"]')).toHaveText("Pump.fun");
  await expect(page.locator('select option[value="polymarket_clob"]')).toHaveText("Polymarket CLOB");
});

test("a degraded feed is disclosed above the table", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/markets") {
      await fulfillJson(route, 200, {
        items: [tickers[0]],
        count: 1,
        fetched_at: "2026-10-07T00:00:01Z",
        from_cache: true,
        feeds: [
          { name: "gamma", ok: true, detail: "", fetched_at: "2026-10-07T00:00:01Z", tickers: 1 },
          { name: "jupiter", ok: false, detail: "upstream timed out", fetched_at: "2026-10-06T23:59:00Z", tickers: 0 },
        ],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Market Screener");
  await expect(page.getByText(/Showing a cached snapshot/)).toBeVisible();
  await expect(page.getByText(/Feed jupiter is not returning data: upstream timed out/)).toBeVisible();
});

test("a 503 market_data_unavailable renders the refusal, not fabricated rows", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/markets") {
      await fulfillJson(route, 503, {
        error: "market_data_unavailable",
        detail: "no live market-data feed returned data for this deployment",
        feeds: [],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Market Screener");
  await expect(page.getByText(/no live market-data feed returned data/)).toBeVisible();
  await expect(page.getByText("No market data is available from the live feeds right now.")).toBeVisible();
  // No table rows are invented.
  await expect(page.locator("tbody tr")).toHaveCount(0);
});
