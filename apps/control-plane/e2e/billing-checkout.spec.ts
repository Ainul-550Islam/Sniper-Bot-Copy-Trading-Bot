/**
 * Billing checkout e2e coverage (GAP MAP v2, Part 5).
 *
 * The billing page fans out five reads (`billing/status`, `usage/limits`,
 * `commercial/state`, `invoices`, `pricing`) and upgrades via
 * `POST /api/saas/checkout`. These specs mock all five plus checkout and
 * assert: plans render from the pricing response, the checkout request
 * carries the plan code + idempotency key, a manual-provider session shows
 * the provider instructions instead of pretending payment happened, and a
 * provider refusal is surfaced honestly.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const plans = [
  {
    code: "starter",
    name: "Starter",
    price_monthly_usd_cents: 2900,
    price_yearly_usd_cents: 29000,
    prices_available: true,
    status: "active",
    description: "Paper trading and limited live modules.",
    features: ["1 strategy", "Paper mode"],
  },
  {
    code: "pro",
    name: "Pro",
    price_monthly_usd_cents: 9900,
    price_yearly_usd_cents: 99000,
    prices_available: true,
    status: "active",
    description: "All trading modules and priority execution.",
    features: ["Unlimited strategies", "Live mode"],
  },
];

function commonMocks(handler?: (pathname: string) => void) {
  return async (route: import("@playwright/test").Route, pathname: string) => {
    if (pathname === "/api/saas/billing/status") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        plan_code: "starter",
        plan_version: 1,
        subscription_status: "active",
        billing_provider: "manual",
        payment_state: null,
        invoice_state: null,
        entitlements_active: true,
        usage: { period: "2026-10", total_requests: 0, total_trades: 0 },
        dunning_state: "none",
        grace_until: null,
        suspension_reason: null,
        as_of: "2026-10-07T00:00:00Z",
      });
      return;
    }
    if (pathname === "/api/saas/usage/limits") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        period: "2026-10",
        plan_code: "starter",
        limits: [],
        as_of: "2026-10-07T00:00:00Z",
      });
      return;
    }
    if (pathname === "/api/saas/commercial/state") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        plan_code: "starter",
        subscription_status: "active",
        billing_provider: "manual",
        entitlements_active: true,
        dunning_state: "none",
        lifecycle_status: "active",
        commercial_consistent: true,
        as_of: "2026-10-07T00:00:00Z",
      });
      return;
    }
    if (pathname === "/api/saas/invoices") {
      await fulfillJson(route, 200, { organization_id: organizationId, invoices: [], count: 0 });
      return;
    }
    if (pathname === "/api/saas/pricing") {
      await fulfillJson(route, 200, { plans, pricing_status: "published" });
      return;
    }
    handler?.(pathname);
    await fulfillJson(route, 200, {});
  };
}

test("checkout posts the plan code with an idempotency key and shows the manual instructions", async ({ page }) => {
  const checkoutBodies: Array<Record<string, unknown>> = [];

  await signIn(page, commonMocks());
  // Replace the default fallthrough with a checkout-aware mock.
  await page.route("**/api/saas/checkout", async (route) => {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    checkoutBodies.push(body);
    await fulfillJson(route, 201, {
      id: "chk_e2e_0001",
      organization_id: organizationId,
      plan_code: body.plan_code,
      provider: "manual",
      status: "pending_manual",
      checkout_url: null,
      instructions: "Reply to billing with reference chk_e2e_0001 to complete payment.",
      expires_at: null,
    });
  });

  await goToViaNav(page, "Billing & Invoices");
  await expect(page.getByText("Starter").first()).toBeVisible();
  await expect(page.getByText("Pro").first()).toBeVisible();

  await page.getByRole("button", { name: "Upgrade to Pro" }).click();

  await expect.poll(() => checkoutBodies.length).toBe(1);
  expect(checkoutBodies[0]).toMatchObject({
    plan_code: "pro",
    provider: "manual",
    idempotency_key: "chk_upgrade_pro",
  });
  await expect(page.getByText(/Checkout session created for Pro \(chk_e2e_0001\)/)).toBeVisible();
  await expect(page.getByText(/reference chk_e2e_0001 to complete payment/)).toBeVisible();
});

test("a checkout refusal is surfaced and the page keeps working", async ({ page }) => {
  await signIn(page, commonMocks());
  await page.route("**/api/saas/checkout", async (route) => {
    await fulfillJson(route, 422, {
      error: "checkout_provider_unavailable",
      reason: "the billing provider is not configured for this organization",
    });
  });

  await goToViaNav(page, "Billing & Invoices");
  await page.getByRole("button", { name: "Upgrade to Pro" }).click();

  await expect(page.getByText(/checkout_provider_unavailable|billing provider is not configured/).first()).toBeVisible();
  // The plan comparison is still usable after the refusal.
  await expect(page.getByRole("button", { name: "Upgrade to Pro" })).toBeEnabled();
});
