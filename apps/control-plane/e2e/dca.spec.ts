/** DCA schedules e2e coverage (Part 02 flows + Part 04, L7).
 * Exercises the DCA surface exactly as it is built: list rows come from the
 * server payload (schedule mint shown via <code>), create posts the wire body
 * `ScheduleForm` produces, and cancellation runs through the inline
 * `CancelConfirm` card (POST /api/tenant/dca-schedules/{id}/cancel per
 * dca-api.ts) — "Keep schedule" sends nothing, "Confirm cancellation" posts.
 * Server refusals must be surfaced verbatim. All API calls mocked.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const mint = "EjhYSpKrtRNPaNjQ7j17wJAgYAgNhLg8nFbnmUV6g2gE";

function schedule(status: string, id = "dca_1") {
  return {
    id,
    mint,
    amount_per_run_sol: 0.5,
    interval_secs: 3600,
    budget_sol: 10,
    spent_sol: 1.5,
    remaining_sol: 8.5,
    status,
    next_run_at: "2026-10-21T08:00:00Z",
    last_run_at: "2026-10-20T07:00:00Z",
    attempts: 3,
    last_error: null,
    created_at: "2026-10-20T08:00:00Z",
  };
}

test("lists schedules from the server, create posts the wire shape, keeping the cancel card sends nothing", async ({ page }) => {
  let createCalls = 0;
  let cancelCalls = 0;
  let created: Record<string, unknown> | null = null;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/dca-schedules" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, schedules: [schedule("active")] });
      return;
    }
    if (pathname === "/api/tenant/dca-schedules" && route.request().method() === "POST") {
      createCalls += 1;
      created = route.request().postDataJSON() as Record<string, unknown>;
      await fulfillJson(route, 201, schedule("active", "dca_2"));
      return;
    }
    if (pathname === "/api/tenant/dca-schedules/dca_1/cancel") {
      cancelCalls += 1;
      await fulfillJson(route, 200, {});
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "DCA Schedules");
  await expect(page.getByText(mint)).toBeVisible();

  await page.getByLabel("Token mint").fill(mint);
  await page.getByLabel("Amount per run (SOL)").fill("0.5");
  await page.getByLabel("Interval (seconds)").fill("3600");
  await page.getByLabel("Total budget (SOL)").fill("10");
  await page.getByRole("button", { name: "Create DCA schedule" }).click();
  await expect.poll(() => createCalls).toBe(1);
  expect(created).toMatchObject({ mint, amount_per_run_sol: 0.5, interval_secs: 3600, budget_sol: 10 });

  // Cancellation is an inline confirm card; "Keep schedule" performs no request.
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await expect(page.getByRole("alertdialog", { name: "Confirm DCA cancellation" })).toBeVisible();
  await page.getByRole("button", { name: "Keep schedule" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(cancelCalls).toBe(0);
});

test("confirmed cancellation posts to the server and the list refetches server truth", async ({ page }) => {
  let cancelCalls = 0;
  let cancelled = false;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/dca-schedules" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        schedules: [schedule(cancelled ? "cancelled" : "active")],
      });
      return;
    }
    if (pathname === "/api/tenant/dca-schedules/dca_1/cancel" && route.request().method() === "POST") {
      cancelCalls += 1;
      cancelled = true;
      await fulfillJson(route, 200, schedule("cancelled"));
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "DCA Schedules");
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await page.getByRole("button", { name: "Confirm cancellation" }).click();
  await expect.poll(() => cancelCalls).toBe(1);
  // The refetched list shows the server's post-cancellation state.
  await expect(page.getByText("cancelled", { exact: true }).first()).toBeVisible();
});

test("server refusal on cancellation is surfaced verbatim", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/tenant/dca-schedules" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, schedules: [schedule("active")] });
      return;
    }
    if (pathname === "/api/tenant/dca-schedules/dca_1/cancel" && route.request().method() === "POST") {
      await fulfillJson(route, 503, { error: "dca_store_unavailable", reason: "schedules are locked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "DCA Schedules");
  await page.getByRole("button", { name: "Cancel", exact: true }).first().click();
  await page.getByRole("button", { name: "Confirm cancellation" }).click();
  await expect(page.getByText("schedules are locked")).toBeVisible();
});
