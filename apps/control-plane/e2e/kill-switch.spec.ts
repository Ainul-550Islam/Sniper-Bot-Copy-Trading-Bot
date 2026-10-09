/**
 * Kill-switch e2e coverage (GAP MAP v2, Part 5).
 *
 * The risk page reads the organization's risk posture from
 * `GET /api/saas/risk-dashboard` and toggles the organization-scoped kill
 * switch via `POST /api/saas/risk-dashboard/kill-switch`. These specs mock
 * both endpoints and assert: (1) the toggle request carries the right body
 * and the UI re-reads the server-confirmed state afterwards, (2) a backend
 * refusal is surfaced verbatim and the UI keeps the last good state, and
 * (3) the confirmation dialog is honoured when declined.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

function riskDashboardState(killSwitchActive: boolean) {
  return {
    organization_id: organizationId,
    kill_switch_active: killSwitchActive,
    reference_asset: "USD",
    max_drawdown_limit_ref: null,
    current_drawdown_ref: null,
    daily_loss_limit_ref: null,
    current_daily_loss_ref: null,
    durable: true,
    modules: [
      { module: "sniper", effective_state: killSwitchActive ? "disabled" : "enabled", override: null },
      { module: "copy", effective_state: killSwitchActive ? "disabled" : "enabled", override: null },
      { module: "polymarket", effective_state: killSwitchActive ? "disabled" : "enabled", override: null },
    ],
    rules: [],
    as_of: "2026-10-07T00:00:00Z",
  };
}

test("activating the kill switch sends the reason and reflects the server-confirmed state", async ({ page }) => {
  let killSwitchActive = false;
  const toggleBodies: Array<Record<string, unknown>> = [];
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/risk-dashboard" && route.request().method() === "GET") {
      await fulfillJson(route, 200, riskDashboardState(killSwitchActive));
      return;
    }
    if (pathname === "/api/saas/risk-dashboard/kill-switch" && route.request().method() === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      toggleBodies.push(body);
      killSwitchActive = body.active === true;
      await fulfillJson(route, 200, {
        success: true,
        kill_switch_active: killSwitchActive,
        updated_at: "2026-10-07T00:00:01Z",
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Risk & Safeguards");
  await expect(page.getByText("STANDBY — NORMAL")).toBeVisible();

  await page.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }).click();

  await expect.poll(() => toggleBodies.length).toBe(1);
  expect(toggleBodies[0]).toMatchObject({ active: true, reason: "Manual operator emergency trigger" });
  await expect(page.getByText("ACTIVE — TRADING BLOCKED")).toBeVisible();
  await expect(page.getByRole("button", { name: "Deactivate & Resume Trading" })).toBeVisible();
});

test("declining the confirmation dialog makes no API call", async ({ page }) => {
  let toggleCalls = 0;
  page.on("dialog", (dialog) => dialog.dismiss());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/risk-dashboard") {
      await fulfillJson(route, 200, riskDashboardState(false));
      return;
    }
    if (pathname === "/api/saas/risk-dashboard/kill-switch") {
      toggleCalls += 1;
      await fulfillJson(route, 200, { success: true, kill_switch_active: true, updated_at: "2026-10-07T00:00:01Z" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Risk & Safeguards");
  await page.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }).click();
  await expect(page.getByText("STANDBY — NORMAL")).toBeVisible();
  expect(toggleCalls).toBe(0);
});

test("a storage refusal is surfaced verbatim and the state is unchanged", async ({ page }) => {
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/risk-dashboard") {
      await fulfillJson(route, 200, riskDashboardState(false));
      return;
    }
    if (pathname === "/api/saas/risk-dashboard/kill-switch") {
      await fulfillJson(route, 503, {
        error: "risk_storage_unavailable",
        reason:
          "tenant kill switch was not changed because the authoritative module-control store was unavailable",
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Risk & Safeguards");
  await page.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }).click();

  await expect(
    page.getByText("tenant kill switch was not changed because the authoritative module-control store was unavailable"),
  ).toBeVisible();
  // The panel keeps the last server-confirmed state: still standby.
  await expect(page.getByText("STANDBY — NORMAL")).toBeVisible();
});
