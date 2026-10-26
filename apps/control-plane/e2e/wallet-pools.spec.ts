/** Wallet pools e2e coverage (Part 02 flows + Part 04, L7).
 * Exercises the wallet-pools page against its real contract:
 * GET /api/saas/wallet-pools returns `{organization_id, pools}` and the
 * auto-selected pool's detail comes back from GET /api/saas/wallet-pools/{id}
 * with `members`; the Part-04 authorization table is driven by GET
 * /api/saas/wallet-access. The page never renders a pool the server did not
 * report, and an unavailable store shows the server's reason verbatim.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn } from "./helpers";

const pool = {
  id: "pool_1",
  name: "E2E routing pool",
  allocation: "round_robin",
  status: "active",
  rotation_cursor: 1,
  active_members: 2,
  created_at: "2026-10-20T08:00:00Z",
};

const member = {
  id: "pm_1",
  weight: 1,
  status: "active",
  logical_identity: "e2e-member-1",
  public_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRjxg8tu9g",
  added_at: "2026-10-20T08:00:00Z",
};

test("lists pools from the server and loads the selected pool's members", async ({ page }) => {
  let detailPath = "";

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/wallet-pools" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, pools: [pool] });
      return;
    }
    if (pathname === "/api/saas/wallet-pools/pool_1" && route.request().method() === "GET") {
      detailPath = pathname;
      await fulfillJson(route, 200, { organization_id: organizationId, ...pool, members: [member] });
      return;
    }
    if (pathname === "/api/saas/custody/signers") {
      await fulfillJson(route, 200, { organization_id: organizationId, signers: [] });
      return;
    }
    if (pathname === "/api/saas/wallet-access") {
      await fulfillJson(route, 200, { organization_id: organizationId, wallets: [] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Wallet Pools");
  await expect(page.getByText("E2E routing pool").first()).toBeVisible();
  expect(detailPath).toBe("/api/saas/wallet-pools/pool_1");
  await expect(page.getByText("e2e-member-1")).toBeVisible();
});

test("authorization table reports honestly when no bindings exist", async ({ page }) => {
  let authorizeCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/wallet-pools" && route.request().method() === "GET") {
      await fulfillJson(route, 200, { organization_id: organizationId, pools: [] });
      return;
    }
    if (pathname === "/api/saas/custody/signers") {
      await fulfillJson(route, 200, { organization_id: organizationId, signers: [] });
      return;
    }
    if (pathname === "/api/saas/wallet-access") {
      await fulfillJson(route, 200, { organization_id: organizationId, wallets: [] });
      return;
    }
    if (pathname === "/api/saas/wallet-access/bind_1/authorize" && route.request().method() === "POST") {
      authorizeCalls += 1;
      await fulfillJson(route, 200, {});
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Wallet Pools");
  // No bindings from GET /api/saas/wallet-access -> the Part-04 table says so
  // plainly instead of showing a fake empty pool of wallets.
  await expect(page.getByText("No wallet bindings were returned for this organization.")).toBeVisible();
  expect(authorizeCalls).toBe(0);
});

test("wallet pool store refusal is surfaced verbatim", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/wallet-pools" && route.request().method() === "GET") {
      await fulfillJson(route, 503, { error: "wallet_store_unavailable", reason: "wallet pools are locked" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Wallet Pools");
  await expect(page.getByText("wallet pools are locked")).toBeVisible();
});
