/** Exports / feeds / wallet-access e2e coverage (Part 04, L5).
 * Exercises the Part-04 read surfaces against the endpoints they actually
 * call: the audit ledger (GET /api/saas/exports?kind=audit) plus its
 * authenticated export download (GET /api/saas/audit/export -> blob, no
 * credentials in a URL), compliance report download (GET
 * /api/saas/reports, GET /api/saas/reports/{id}/download), the org events
 * feed (GET /api/saas/events) with its honest empty state, the backup-status
 * card on /status (GET /api/saas/backup/status), read-only notification
 * preferences (GET /api/saas/notifications/preferences — the only verb the
 * contract exposes), and wallet-access authorization (POST
 * /api/saas/wallet-access/{id}/authorize) gated behind window.confirm.
 * All API calls mocked; no live backend.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, signIn, testWalletBinding } from "./helpers";

const baseMock = {
  "/api/saas/wallet-pools": { organization_id: organizationId, pools: [] },
  "/api/saas/custody/signers": { organization_id: organizationId, signers: [] },
  "/api/saas/wallet-access": { organization_id: organizationId, wallets: [testWalletBinding] },
};

test("audit export downloads the server envelope via an authenticated fetch", async ({ page }) => {
  let exportCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/exports" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        kind: "audit",
        generated_at: "2026-10-20T08:00:00Z",
        data: {
          records: [
            {
              id: "aud_1",
              actor: "operator@example.test",
              action: "settings.update",
              target: "organization settings",
              at: "2026-10-20T08:00:00Z",
            },
          ],
        },
      });
      return;
    }
    if (pathname === "/api/saas/audit/export" && route.request().method() === "GET") {
      exportCalls += 1;
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        records: [{ id: "aud_1", action: "settings.update", at: "2026-10-20T08:00:00Z" }],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Audit Logs");
  await expect(page.getByText("settings.update")).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export Audit" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("sniper-audit-export.json");
  await expect.poll(() => exportCalls).toBe(1);
});

test("compliance report download fetches the report download path", async ({ page }) => {
  let reportCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/reports" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        items: [
          {
            id: "rep_1",
            name: "October accounting ledger",
            type: "accounting_ledger",
            format: "csv",
            size_bytes: 20480,
            record_count: 312,
            created_at: "2026-10-20T08:00:00Z",
            download_url: "/api/saas/reports/rep_1/download",
          },
        ],
      });
      return;
    }
    if (pathname === "/api/saas/reports/rep_1/download" && route.request().method() === "GET") {
      reportCalls += 1;
      await fulfillJson(route, 200, { id: "rep_1", kind: "accounting_ledger", rows: [] });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Reports & Compliance");
  await expect(page.getByText("October accounting ledger")).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download" }).first().click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("sniper-report-rep_1.json");
  await expect.poll(() => reportCalls).toBe(1);
});

test("notification preferences are read-only GET only", async ({ page }) => {
  let prefCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/notifications/preferences" && route.request().method() === "GET") {
      prefCalls += 1;
      await fulfillJson(route, 200, { organization_id: organizationId, items: [{ channel: "email", enabled: true, detail: null }], updated_at: "2026-10-20T08:00:00Z" });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Notifications");
  await expect(page.getByText(/email/i)).toBeVisible();
  expect(prefCalls).toBe(1);
});

test("events feed fetched from server, empty state honest", async ({ page }) => {
  let eventsCalls = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/events" && route.request().method() === "GET") {
      eventsCalls += 1;
      await fulfillJson(route, 200, { organization_id: organizationId, events: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Activity Timeline");
  await expect(page.getByText("No organization events were returned.")).toBeVisible();
  await expect.poll(() => eventsCalls).toBe(1);
});

test("backup status card shown on status page", async ({ page }) => {
  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/status" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        overall_status: "operational",
        as_of: "2026-10-20T08:00:00Z",
        components: [
          { name: "api", category: "service", status: "operational", latency_ms: 25, last_checked: "2026-10-20T08:00:00Z", details: "healthy" },
        ],
        active_incidents_count: 0,
      });
      return;
    }
    if (pathname === "/api/saas/backup/status" && route.request().method() === "GET") {
      await fulfillJson(route, 200, {
        backup_count: 3,
        last_backup_at: "2026-10-20T07:00:00Z",
        last_verified_restore: "2026-10-19T07:00:00Z",
        encrypted: true,
        offsite: { state: "synced", last_sync_at: "2026-10-20T07:05:00Z" },
        as_of: "2026-10-20T08:00:00Z",
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "System Status");
  await expect(page.getByText("Backup Status")).toBeVisible();
  await expect(page.getByText("Backups", { exact: true })).toBeVisible();
  await expect(page.getByText("synced")).toBeVisible();
});

test("wallet-access authorize confirm declined sends no POST", async ({ page }) => {
  let authorizeCalls = 0;
  page.on("dialog", (dialog) => dialog.dismiss());

  await signIn(page, async (route, pathname) => {
    const fixed = (baseMock as Record<string, unknown>)[pathname];
    if (fixed !== undefined) {
      await fulfillJson(route, 200, fixed);
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
  await expect(page.getByText(testWalletBinding.public_address)).toBeVisible();
  await page.getByRole("button", { name: "Authorize" }).first().click();
  await page.waitForTimeout(250);
  expect(authorizeCalls).toBe(0);
  // Declining left the binding exactly as the server reported it.
  await expect(page.getByText("E2E treasury wallet")).toBeVisible();
});

test("wallet-access authorize confirm accepted sends POST with module body", async ({ page }) => {
  let authorizeCalls = 0;
  let body: Record<string, unknown> | null = null;
  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    const fixed = (baseMock as Record<string, unknown>)[pathname];
    if (fixed !== undefined) {
      await fulfillJson(route, 200, fixed);
      return;
    }
    if (pathname === "/api/saas/wallet-access/bind_1/authorize" && route.request().method() === "POST") {
      authorizeCalls += 1;
      body = route.request().postDataJSON() as Record<string, unknown>;
      await fulfillJson(route, 200, { id: "bind_1", module: "module.sniper", authorized: true });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Wallet Pools");
  await page.getByRole("button", { name: "Authorize" }).first().click();
  await expect.poll(() => authorizeCalls).toBe(1);
  expect(body).toMatchObject({ module: "module.sniper" });
});
