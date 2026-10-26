/** Organization lifecycle e2e coverage (Part 04, L2).
 * Exercises the OrgLifecyclePanel mounted on /settings/organization: the
 * server-confirmed state rendering (exact "State: <x>" line), suspend/resume
 * gated behind window.confirm, and close gated behind the typed-slug
 * confirmation. The page owns the GET /api/saas/organizations/{id}/lifecycle
 * fetch and refetches after every accepted action, so these tests mock a
 * stateful lifecycle endpoint and assert the UI follows the server.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

function lifecycleBody(state: string) {
  return {
    organization_id: organizationId,
    lifecycle_status: state,
    state,
    reason: state === "suspended" ? "operator requested" : null,
    suspended_at: state === "suspended" ? "2026-10-20T09:00:00Z" : null,
    suspended_by: state === "suspended" ? "operator@example.test" : null,
    closed_at: null,
    updated_at: "2026-10-20T09:00:00Z",
  };
}

test("displays server-confirmed lifecycle state", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody("suspended"));
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  // The panel renders one "State: <x>" line; match it exactly so the assert
  // proves the panel (not a sibling stat card) shows the server value.
  await expect(page.getByText("State: suspended")).toBeVisible();
});

test("close requires exact typed slug; wrong slug keeps button disabled", async ({ page }) => {
  let closeCalls = 0;
  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody("active"));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/close` && route.request().method() === "POST") {
      closeCalls += 1;
      await fulfillJson(route, 200, { organization_id: organizationId, status: "close_scheduled", reason: "customer requested", updated_at: "2026-10-20T09:00:00Z" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  const input = page.getByLabel(/type e2e-tenant to confirm/i);
  await input.fill("wrong-slug");
  await expect(page.getByRole("button", { name: "Confirm close organization" })).toBeDisabled();
  await input.fill("e2e-tenant");
  await expect(page.getByRole("button", { name: "Confirm close organization" })).toBeEnabled();
  await page.getByRole("button", { name: "Confirm close organization" }).click();
  await expect.poll(() => closeCalls).toBe(1);
});

test("suspend and resume confirm before posting", async ({ page }) => {
  let suspendCalls = 0;
  let resumeCalls = 0;
  let state = "active";
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody(state));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/suspend` && route.request().method() === "POST") {
      suspendCalls += 1;
      state = "suspended";
      await fulfillJson(route, 200, lifecycleBody(state));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/resume` && route.request().method() === "POST") {
      resumeCalls += 1;
      state = "active";
      await fulfillJson(route, 200, lifecycleBody(state));
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  await page.getByRole("button", { name: "Suspend organization" }).click();
  await expect.poll(() => suspendCalls).toBe(1);
  // After the refetch the panel shows the server's suspended state, which is
  // what unlocks the resume control.
  await expect(page.getByText("State: suspended")).toBeVisible();
  await page.getByRole("button", { name: "Resume organization" }).click();
  await expect.poll(() => resumeCalls).toBe(1);
  await expect(page.getByText("State: active")).toBeVisible();
});

test("declined confirmation sends no suspend request", async ({ page }) => {
  let suspendCalls = 0;
  page.on("dialog", (dialog) => dialog.dismiss());

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody("active"));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/suspend` && route.request().method() === "POST") {
      suspendCalls += 1;
      await fulfillJson(route, 200, {});
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  await page.getByRole("button", { name: "Suspend organization" }).click();
  await page.waitForTimeout(250);
  expect(suspendCalls).toBe(0);
  await expect(page.getByText("State: active")).toBeVisible();
});
