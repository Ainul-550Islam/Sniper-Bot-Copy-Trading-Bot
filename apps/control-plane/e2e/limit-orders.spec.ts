/** Limit orders e2e coverage (Part 02 flows + Part 04, L7).
 * Exercises the limit-orders surface exactly as it is built: list rows from
 * the server's `{organization_id, orders, next_before}` envelope, create posts
 * the wire body `OrderForm` produces, and cancellation runs through the inline
 * `CancelConfirm` card (POST /api/tenant/limit-orders/{id}/cancel per
 * limit-orders-api.ts) — "Keep order" sends nothing, "Confirm cancellation"
 * posts. Server refusals are surfaced verbatim. All API calls mocked.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const mint = "EjhYSpKrtRNPaNjQ7j17wJAgYAgNhLg8nFbnmUV6g2gE";

function order(status: string, id = "lo_1") {
  return {
    id,
    mint,
    side: "buy",
    trigger: "at_or_above",
    price_sol: 1.5,
    amount: 2,
    status,
    created_at: "2026-10-20T08:00:00Z",
    expires_at: null,
    resolved_at: null,
    attempts: 0,
    last_error: null,
  };
}

test("lists orders from the server, create posts the wire shape, keeping the cancel card sends nothing", async ({ page }) => {
  let createCalls = 0;
  let cancelCalls = 0;
  let created: Record<string, unknown> | null = null;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/limit-orders" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, orders: [order("active")], next_before: null });
      return;
    }
    if (pathname === "/api/tenant/limit-orders" && route.request().method() === "POST") {
      createCalls += 1;
      created = route.request().postDataJSON() as Record<string, unknown>;
      await fulfillJson(route, 201, order("active", "lo_2"));
      return;
    }
    if (pathname === "/api/tenant/limit-orders/lo_1/cancel") {
      cancelCalls += 1;
      await fulfillJson(route, 200, {});
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Limit Orders");
  await expect(page.getByText(mint)).toBeVisible();

  await page.getByLabel("Token mint").fill(mint);
  await page.getByLabel("Trigger", { exact: true }).selectOption("at_or_above");
  await page.getByLabel("Trigger price (SOL)").fill("1.5");
  await page.getByLabel("Amount", { exact: true }).fill("2");
  await page.getByRole("button", { name: "Create limit order" }).click();
  await expect.poll(() => createCalls).toBe(1);
  expect(created).toMatchObject({ mint, side: "buy", trigger: "at_or_above", price_sol: 1.5, amount: 2 });

  // Cancellation is an inline confirm card; "Keep order" performs no request.
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await expect(page.getByRole("alertdialog", { name: "Confirm limit order cancellation" })).toBeVisible();
  await page.getByRole("button", { name: "Keep order" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(cancelCalls).toBe(0);
});

test("confirmed cancellation posts to the server and the list refetches server truth", async ({ page }) => {
  let cancelCalls = 0;
  let cancelled = false;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/limit-orders" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        orders: [order(cancelled ? "cancelled" : "active")],
        next_before: null,
      });
      return;
    }
    if (pathname === "/api/tenant/limit-orders/lo_1/cancel" && route.request().method() === "POST") {
      cancelCalls += 1;
      cancelled = true;
      await fulfillJson(route, 200, { order: order("cancelled") });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Limit Orders");
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await page.getByRole("button", { name: "Confirm cancellation" }).click();
  await expect.poll(() => cancelCalls).toBe(1);
  // The refetched list shows the server's post-cancellation status.
  await expect(page.getByText("cancelled", { exact: true }).first()).toBeVisible();
});

test("server refusal on cancellation is surfaced verbatim", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/limit-orders" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, orders: [order("active")], next_before: null });
      return;
    }
    if (pathname === "/api/tenant/limit-orders/lo_1/cancel" && route.request().method() === "POST") {
      await fulfillJson(route, 503, { error: "order_store_unavailable", reason: "orders are locked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Limit Orders");
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await page.getByRole("button", { name: "Confirm cancellation" }).click();
  await expect(page.getByText("orders are locked")).toBeVisible();
});
