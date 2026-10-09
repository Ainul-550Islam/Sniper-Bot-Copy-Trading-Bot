/**
 * Shared fixtures for control-plane e2e specs (GAP MAP v2, Part 5).
 *
 * Every spec runs against `next dev` with the SaaS API fully mocked at the
 * network boundary (`page.route`). The mocks return REALISTIC shapes taken
 * from the wire contract (openapi.json) but the data is clearly test data:
 * none of it is ever presented as live trading results, and the specs assert
 * the UI's HONEST states (empty lists, refusals surfaced verbatim) rather
 * than fabricated success.
 */
import { expect, type Page, type Route } from "@playwright/test";

export const organizationId = "2bd6a235-b1f5-4aa4-9d66-09c74ec88d61";

export const testUser = {
  id: "2c9b7673-d034-4ccb-a591-d8a1ba48b741",
  email: "operator@example.test",
  email_verified: true,
  display_name: "Test Operator",
  status: "active",
  platform_admin: false,
  created_at: "2026-01-01T00:00:00.000Z",
  last_login_at: null,
};

export const testOrganization = {
  organization_id: organizationId,
  slug: "e2e-tenant",
  name: "E2E Tenant",
  status: "active",
  role: "owner",
  membership_status: "active",
};

export function sessionPayload(expiresInMinutes = 60) {
  return {
    id: "a04591cf-0e0d-49c5-b865-a79a36326415",
    prefix: "ses_e2e",
    expires_at: new Date(Date.now() + expiresInMinutes * 60_000).toISOString(),
    organization_id: organizationId,
  };
}

export async function fulfillJson(route: Route, status: number, body: unknown) {
  await route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

/**
 * Route every `/api/**` request through `handler`. Specs inspect the pathname
 * and fulfill what they care about; anything unhandled is answered with an
 * empty 200 so incidental background calls do not fail the run.
 */
export async function installApiMock(
  page: Page,
  handler: (route: Route, pathname: string) => Promise<void> | void,
) {
  await page.route("**/api/**", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    await handler(route, pathname);
  });
}

/**
 * Sign the test user in against a mocked `/api/saas/sessions` and land inside
 * the authenticated shell. `onRequest` lets a spec observe/collect the login
 * and any follow-up calls.
 */
export async function signIn(
  page: Page,
  handler: (route: Route, pathname: string) => Promise<void> | void,
) {
  await installApiMock(page, async (route, pathname) => {
    if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
      await fulfillJson(route, 200, {
        user: testUser,
        token: "e2e-session-token",
        session: sessionPayload(),
        organization_id: organizationId,
        organization_slug: testOrganization.slug,
      });
      return;
    }
    if (pathname === "/api/saas/users/me") {
      await fulfillJson(route, 200, { user: testUser, organizations: [testOrganization] });
      return;
    }
    await handler(route, pathname);
  });

  await page.goto("/");
  await page.getByLabel("Email").fill(testUser.email);
  await page.getByLabel("Password").fill("correct-password");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();
}

/** Navigate inside the SPA (the session lives in memory; a hard reload drops it). */
export async function goToViaNav(page: Page, label: string) {
  await page.getByRole("navigation", { name: "Sections" }).getByRole("link", { name: label }).click();
}
