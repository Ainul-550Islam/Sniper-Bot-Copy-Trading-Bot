/** Billing status / reconcile / usage limits e2e coverage (Part 04, L3).
 * Exercises the Part-04 panels mounted on the billing page against the real
 * org-scoped contract (GET /api/saas/billing/status/{id}, GET
 * /api/saas/usage/limits/{id}, GET /api/saas/billing/reconcile/{id}, POST
 * /api/saas/billing/reconcile gated behind window.confirm) — while also
 * feeding the page's own Part-02 commercial calls ({billing/status,
 * usage/limits, commercial/state, invoices, pricing}) their documented
 * shapes, because those panels only render when the surrounding page mounts.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const billingStatusBody = {
  organization_id: organizationId,
  plan_code: "pro",
  plan_version: 3,
  subscription_status: "active",
  billing_provider: "stripe",
  payment_state: null,
  invoice_state: null,
  entitlements_active: true,
  usage: { period: "2026-10", total_requests: 1240, total_trades: 86 },
  dunning_state: "current",
  grace_until: null,
  suspension_reason: null,
  as_of: "2026-10-20T08:00:00Z",
};

const usageLimitsBody = {
  organization_id: organizationId,
  period: "2026-10",
  plan_code: "pro",
  limits: [
    { feature: "api_requests", state: "within_allowance", limit: 10000, current: 1240, remaining: 8760, allows: true },
    { feature: "trades_per_month", state: "soft_limit_exceeded", limit: 100, current: 105, remaining: null, allows: true },
  ],
  as_of: "2026-10-20T08:00:00Z",
};

const plansBody = [
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

const reconcileResultBody = {
  organization_id: organizationId,
  action: "update",
  reason: "Provider entitlement aligned with local state",
  decided_at: "2026-10-20T09:00:00Z",
  idempotent: true,
  internal_subscription_status: "active",
  provider_event_kind: "subscription_updated",
};

/** The billing page's own Part-02 data layer; without these the page cannot
 *  mount and no panel assertion would be meaningful. */
function basePageMock(pathname: string): unknown | undefined {
  switch (pathname) {
    case "/api/saas/billing/status":
      return { ...billingStatusBody, plan_code: "starter", billing_provider: "manual", dunning_state: "none" };
    case "/api/saas/usage/limits":
      return { organization_id: organizationId, period: "2026-10", plan_code: "starter", limits: [], as_of: "2026-10-20T08:00:00Z" };
    case "/api/saas/commercial/state":
      return {
        organization_id: organizationId,
        plan_code: "starter",
        subscription_status: "active",
        billing_provider: "manual",
        entitlements_active: true,
        dunning_state: "none",
        lifecycle_status: "active",
        commercial_consistent: true,
        as_of: "2026-10-20T08:00:00Z",
      };
    case "/api/saas/invoices":
      return { organization_id: organizationId, invoices: [], count: 0 };
    case "/api/saas/pricing":
      return { plans: plansBody, pricing_status: "published" };
    default:
      return undefined;
  }
}

test("panels display server billing status and usage limits", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/billing/status/${organizationId}` && route.request().method() === "GET") {
      await fulfillJson(route, 200, billingStatusBody);
      return;
    }
    if (pathname === `/api/saas/usage/limits/${organizationId}` && route.request().method() === "GET") {
      await fulfillJson(route, 200, usageLimitsBody);
      return;
    }
    const base = basePageMock(pathname);
    if (base !== undefined) {
      await fulfillJson(route, 200, base);
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Billing & Invoices");
  // The Part-04 panel shows the org-scoped status the page's own base call
  // does not carry: plan tier "PRO" with the entitlement tag.
  await expect(page.getByText("PRO", { exact: true })).toBeVisible();
  await expect(page.getByText("Entitlements Active").first()).toBeVisible();
  // And the org-scoped usage limits table renders the server rows verbatim.
  await expect(page.getByText("api_requests")).toBeVisible();
  await expect(page.getByText("trades_per_month")).toBeVisible();
  await expect(page.getByText("soft_limit_exceeded")).toBeVisible();
});

test("reconcile posts after confirmation, then refetches the result", async ({ page }) => {
  let reconcileCalls = 0;
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/billing/status/${organizationId}` && route.request().method() === "GET") {
      await fulfillJson(route, 200, billingStatusBody);
      return;
    }
    if (pathname === "/api/saas/billing/reconcile" && route.request().method() === "POST") {
      reconcileCalls += 1;
      await fulfillJson(route, 200, reconcileResultBody);
      return;
    }
    if (pathname === `/api/saas/billing/reconcile/${organizationId}` && route.request().method() === "GET") {
      await fulfillJson(route, 200, reconcileResultBody);
      return;
    }
    const base = basePageMock(pathname);
    if (base !== undefined) {
      await fulfillJson(route, 200, base);
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Billing & Invoices");
  await page.getByRole("button", { name: "Reconcile with Provider" }).click();
  await expect.poll(() => reconcileCalls).toBe(1);
  await expect(page.getByText(/Reconciliation accepted/)).toBeVisible();
  await expect(page.getByText("Provider entitlement aligned with local state")).toBeVisible();
  await expect(page.getByText("update", { exact: true })).toBeVisible();
});

test("reconcile confirm declined sends no POST", async ({ page }) => {
  let reconcileCalls = 0;
  page.on("dialog", (dialog) => dialog.dismiss());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/billing/reconcile" && route.request().method() === "POST") {
      reconcileCalls += 1;
      await fulfillJson(route, 200, {});
      return;
    }
    const base = basePageMock(pathname);
    if (base !== undefined) {
      await fulfillJson(route, 200, base);
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Billing & Invoices");
  await page.getByRole("button", { name: "Reconcile with Provider" }).click();
  await page.waitForTimeout(250);
  expect(reconcileCalls).toBe(0);
});
