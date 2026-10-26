/** Team invite revoke / resend e2e coverage (Part 04, L1).
 * Exercises the pending-invite list on /settings/team against the real
 * team-invites-api contract: GET /api/saas/team/invites returns
 * `{organization_id, items, count}`; Resend POSTs
 * /api/saas/team/invites/{id}/resend; Revoke is destructive and gated behind
 * window.confirm, with DELETE /api/saas/team/invites/{id} only after
 * acceptance. The page refetches after every action, so the mocked list is
 * stateful and the assertions prove the UI follows the server.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const invite = {
  id: "inv_1",
  email: "invitee@example.test",
  role: "operator",
  status: "pending",
  created_at: "2026-10-20T08:00:00Z",
  expires_at: null,
};

test("lists pending invites and resend posts the exact path; declined revoke sends nothing", async ({ page }) => {
  let resendCalls = 0;
  let revokeCalls = 0;
  page.on("dialog", (dialog) => dialog.dismiss());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/team/invites" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [invite], count: 1 });
      return;
    }
    if (pathname === `/api/saas/team/invites/${invite.id}/resend` && route.request().method() === "POST") {
      resendCalls += 1;
      await fulfillJson(route, 200, { id: invite.id, status: "pending", resent_at: "2026-10-20T09:00:00Z" });
      return;
    }
    if (pathname === `/api/saas/team/invites/${invite.id}` && route.request().method() === "DELETE") {
      revokeCalls += 1;
      await fulfillJson(route, 200, { id: invite.id, status: "revoked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Team Members");
  await expect(page.getByText(invite.email)).toBeVisible();

  // Resend: POST to the exact resend path; the invite stays pending until the
  // server says otherwise, so the row must not disappear or fake a new state.
  await page.getByRole("button", { name: "Resend" }).first().click();
  await expect.poll(() => resendCalls).toBe(1);
  await expect(page.getByText(invite.email)).toBeVisible();

  // Revoke: confirm declined -> no DELETE is ever sent.
  await page.getByRole("button", { name: "Revoke" }).first().click();
  await page.waitForTimeout(250);
  expect(revokeCalls).toBe(0);
  await expect(page.getByText(invite.email)).toBeVisible();
});

test("revoke confirm accepted sends DELETE and refetches", async ({ page }) => {
  let revokeCalls = 0;
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/team/invites" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        items: revokeCalls > 0 ? [] : [invite],
        count: revokeCalls > 0 ? 0 : 1,
      });
      return;
    }
    if (pathname === `/api/saas/team/invites/${invite.id}` && route.request().method() === "DELETE") {
      revokeCalls += 1;
      await fulfillJson(route, 200, { id: invite.id, status: "revoked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Team Members");
  await expect(page.getByText(invite.email)).toBeVisible();
  await page.getByRole("button", { name: "Revoke" }).first().click();
  await expect.poll(() => revokeCalls).toBe(1);
  await expect(page.getByText("No pending invitations.")).toBeVisible();
});

test("revoke server refusal is surfaced verbatim", async ({ page }) => {
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/team/invites" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, items: [invite], count: 1 });
      return;
    }
    if (pathname === `/api/saas/team/invites/${invite.id}` && route.request().method() === "DELETE") {
      await fulfillJson(route, 503, { error: "team_store_unavailable", reason: "invites are locked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Team Members");
  await expect(page.getByText(invite.email)).toBeVisible();
  await page.getByRole("button", { name: "Revoke" }).first().click();
  await expect(page.getByText("invites are locked")).toBeVisible();
  // The refusal must not make the row vanish — the list stays server truth.
  await expect(page.getByText(invite.email)).toBeVisible();
});
