import { expect, test } from "@playwright/test";
import { fulfillJson, goToViaNav, organizationId, sessionPayload, signIn } from "./helpers";

const now = "2026-10-30T00:00:00.000Z";
const currentSession = sessionPayload();
const remoteSessionId = "bd2c5f70-d82b-4fbe-8f08-dc2f44a9c112";

test("security page shows MFA status, lists server sessions, and revokes only the selected user's session", async ({ page }) => {
  const requests: string[] = [];
  const remoteSession = {
    id: remoteSessionId,
    prefix: "ses_remote",
    user_agent: "Firefox on Linux",
    ip: "203.0.113.19",
    created_at: now,
    last_seen_at: now,
    expires_at: "2026-10-30T12:00:00.000Z",
    revoked_at: null as string | null,
    state: "active",
    current: false,
  };
  const current = {
    id: currentSession.id,
    prefix: currentSession.prefix,
    user_agent: "Chromium on Linux",
    ip: "203.0.113.20",
    created_at: now,
    last_seen_at: now,
    expires_at: currentSession.expires_at,
    revoked_at: null,
    state: "active",
    current: true,
  };

  page.on("dialog", (dialog) => dialog.accept());

  await signIn(page, async (route, pathname) => {
    const method = route.request().method();
    requests.push(`${method} ${pathname}`);
    if (pathname === "/api/saas/security/status" && method === "GET") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        mfa_enforced: true,
        has_totp_configured: true,
        ip_allowlist: ["192.0.2.0/24"],
        ip_allowlist_count: 1,
        session_duration_hours: 12,
        require_signed_commits: false,
      });
      return;
    }
    if (pathname === "/api/saas/users/me/sessions" && method === "GET") {
      await fulfillJson(route, 200, { items: [current, remoteSession], count: 2 });
      return;
    }
    if (pathname === `/api/saas/users/me/sessions/${remoteSessionId}` && method === "DELETE") {
      remoteSession.state = "revoked";
      remoteSession.revoked_at = now;
      await fulfillJson(route, 200, { ok: true, session_id: remoteSessionId, revoked: true });
      return;
    }
    if (pathname === "/api/saas/users/me/logout" && method === "POST") {
      await fulfillJson(route, 200, { success: true });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Security & Posture");
  await expect(page.getByRole("heading", { name: "Security Posture & Access Controls" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable Mandatory MFA" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Sessions" })).toBeVisible();
  await expect(page.getByText("ses_e2e")).toBeVisible();
  await expect(page.getByText("ses_remote")).toBeVisible();
  await expect(page.getByText("Firefox on Linux")).toBeVisible();
  expect(requests).toContain("GET /api/saas/users/me/sessions");

  await page.getByRole("button", { name: "Revoke session ses_remote" }).click();
  await expect(page.getByText("Status: revoked")).toBeVisible();
  expect(requests).toContain(`DELETE /api/saas/users/me/sessions/${remoteSessionId}`);

  await page.getByRole("button", { name: "Sign out this session" }).click();
  await expect(page.getByRole("heading", { name: "Session ended" })).toBeVisible();
  expect(requests).toContain("POST /api/saas/users/me/logout");
});

test("session inventory exposes loading and empty states", async ({ page }) => {
  let releaseSessions: (() => void) | undefined;
  let signalSessionsRequest: (() => void) | undefined;
  const sessionsRequested = new Promise<void>((resolve) => {
    signalSessionsRequest = resolve;
  });

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/security/status") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        mfa_enforced: false,
        has_totp_configured: false,
        ip_allowlist: [],
        ip_allowlist_count: 0,
        session_duration_hours: 24,
        require_signed_commits: false,
      });
      return;
    }
    if (pathname === "/api/saas/users/me/sessions") {
      signalSessionsRequest?.();
      await new Promise<void>((resolve) => {
        releaseSessions = resolve;
      });
      await fulfillJson(route, 200, { items: [], count: 0 });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Security & Posture");
  await sessionsRequested;
  await expect(page.getByText("Loading sessions…")).toBeVisible();
  expect(releaseSessions).toBeDefined();
  releaseSessions?.();
  await expect(page.getByText("No sessions were returned for this account.")).toBeVisible();
});

test("session inventory surfaces load errors and retries against the server", async ({ page }) => {
  let sessionReads = 0;

  await signIn(page, async (route, pathname) => {
    if (pathname === "/api/saas/security/status") {
      await fulfillJson(route, 200, {
        organization_id: organizationId,
        mfa_enforced: false,
        has_totp_configured: false,
        ip_allowlist: [],
        ip_allowlist_count: 0,
        session_duration_hours: 24,
        require_signed_commits: false,
      });
      return;
    }
    if (pathname === "/api/saas/users/me/sessions") {
      sessionReads += 1;
      await fulfillJson(route, 503, {
        error: "session_inventory_unavailable",
        reason: "session inventory temporarily unavailable",
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await goToViaNav(page, "Security & Posture");
  const inventory = page.locator('section[aria-labelledby="session-inventory-heading"]');
  await expect(inventory.getByRole("alert")).toContainText("session inventory temporarily unavailable");
  await expect(inventory.getByRole("button", { name: "Retry session list" })).toBeVisible();
  await inventory.getByRole("button", { name: "Retry session list" }).click();
  await expect.poll(() => sessionReads).toBe(2);
  await expect(inventory.getByRole("alert")).toContainText("session inventory temporarily unavailable");
});
