/** Custody actions e2e coverage (Part 02 flows + Part 04, L7).
 * Exercises the custody page against its real contract:
 * GET /api/saas/custody/profiles (`{organization_id, profiles}`) and GET
 * /api/saas/custody/signers (`{organization_id, signers}`). The page renders
 * provider-reported profile status and the signer inventory verbatim; a
 * refused profile fetch must surface the server's reason text, not a
 * fabricated healthy state. All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const signer = {
  id: "sg_1",
  logical_identity: "e2e-signer-1",
  public_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRjxg8tu9g",
  provider_type: "fireblocks",
  status: "active",
  capabilities: ["withdraw"],
};

const profile = {
  id: "cp_1",
  organization_id: organizationId,
  name: "E2E treasury custody",
  provider_type: "fireblocks",
  status: "active",
  public_signers: [signer],
  created_at: "2026-10-20T08:00:00Z",
};

test("displays custody profiles and signers reported by the server", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/custody/profiles" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, profiles: [profile] });
      return;
    }
    if (pathname === "/api/saas/custody/signers" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, signers: [signer] });
      return;
    }
    if (pathname === `/api/saas/custody/profiles/${profile.id}/signers` && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, profile_id: profile.id, signers: [signer] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Custody");
  // The profile name renders in cards, selects and tables; the signer table
  // row shows its public address. Assert the server values appear.
  await expect(page.getByText("E2E treasury custody").first()).toBeVisible();
  await expect(
    page.locator("td code", { hasText: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRjxg8tu9g" }).first(),
  ).toBeVisible();
  await expect(page.getByText("fireblocks").first()).toBeVisible();
});

test("custody server refusal is surfaced verbatim", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/custody/profiles" && route.request().method() === "GET") {
      await fulfillJson(route, 503, { error: "custody_store_unavailable", reason: "custody provider is unreachable" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Custody");
  await expect(page.getByText("custody provider is unreachable").first()).toBeVisible();
});
