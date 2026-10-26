/** Lifecycle action refusal/gating e2e coverage (Part 04, L6).
 * Complements e2e/org-lifecycle.spec.ts (which covers the accepted happy
 * paths) with the two behaviours that protect tenants from destructive
 * mistakes: a refused suspend must be surfaced verbatim, and Resume must
 * remain non-actionable until the SERVER reports a suspended state — the
 * UI never synthesizes the transition. All API calls mocked; no live backend.
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

test("a refused suspension is surfaced verbatim and does not fake the state", async ({ page }) => {
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody("active"));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/suspend` && route.request().method() === "POST") {
      await fulfillJson(route, 403, { error: "suspension_denied", reason: "retention hold active" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  await page.getByRole("button", { name: "Suspend organization" }).click();
  await expect(page.getByText("retention hold active")).toBeVisible();
  // The refusal means nothing changed: the panel still shows the server state.
  await expect(page.getByText("State: active")).toBeVisible();
});

test("resume stays non-actionable until the server reports a suspended organization", async ({ page }) => {
  let resumeCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody("active"));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/resume` && route.request().method() === "POST") {
      resumeCalls += 1;
      await fulfillJson(route, 200, lifecycleBody("active"));
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Organization");
  await expect(page.getByRole("button", { name: "Not suspended" })).toBeDisabled();
  expect(resumeCalls).toBe(0);
});

test("after a suspension the panel refetches and only then unlocks resume", async ({ page }) => {
  let state = "active";
  let resumeCalls = 0;
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === `/api/saas/organizations/${organizationId}/lifecycle` && route.request().method() === "GET") {
      await fulfillJson(route, 200, lifecycleBody(state));
      return;
    }
    if (pathname === `/api/saas/organizations/${organizationId}/suspend` && route.request().method() === "POST") {
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
  await expect(page.getByRole("button", { name: "Not suspended" })).toBeDisabled();
  await page.getByRole("button", { name: "Suspend organization" }).click();
  await expect(page.getByText("State: suspended")).toBeVisible();
  await expect(page.getByRole("button", { name: "Resume organization" })).toBeEnabled();
  // No resume is attempted on its own; only an explicit click posts.
  expect(resumeCalls).toBe(0);
});
