/**
 * Auth-flow e2e coverage (GAP MAP v2, Part 5).
 *
 * Covers: sign-up consent gating (registration refuses to submit until the
 * legal bundle is accepted, and the consent record is transmitted), the
 * forgot-password request page, the reset-password confirm page (valid and
 * refused tokens), and the verify-email page (token confirm + resend). All
 * backend behaviour is mocked at the network boundary with wire-contract
 * shapes; nothing here fabricates live results.
 */
import { expect, test } from "@playwright/test";
import { fulfillJson, testUser } from "./helpers";

test("registration is blocked until the legal consent checkbox is ticked, then consent is sent", async ({ page }) => {
  const registerBodies: Array<Record<string, unknown>> = [];
  await page.route("**/api/saas/**", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === "/api/saas/users" && route.request().method() === "POST") {
      registerBodies.push(route.request().postDataJSON() as Record<string, unknown>);
      await fulfillJson(route, 201, {
        organization_id: "11111111-2222-3333-4444-555555555555",
        slug: "new-tenant",
        name: "New Tenant",
      });
      return;
    }
    if (pathname === "/api/saas/users/me") {
      await fulfillJson(route, 200, {
        user: testUser,
        organizations: [
          {
            organization_id: "11111111-2222-3333-4444-555555555555",
            slug: "new-tenant",
            name: "New Tenant",
            status: "active",
            role: "owner",
            membership_status: "active",
          },
        ],
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await page.goto("/");
  await page.getByRole("tab", { name: "Create account" }).click();

  await page.getByLabel("Display name").fill("E2E Signer");
  await page.getByLabel("Email").fill("new.operator@example.test");
  await page.getByLabel("Password").fill("correct-horse-battery-staple");

  // Before consent: the submit button is disabled and the hint explains why.
  await expect(page.getByRole("button", { name: "Create account" })).toBeDisabled();
  await expect(page.getByText("Accept the legal documents above to enable account creation.")).toBeVisible();

  await page.getByRole("checkbox").check();
  await expect(page.getByRole("button", { name: "Create account" })).toBeEnabled();
  await page.getByRole("button", { name: "Create account" }).click();

  await expect.poll(() => registerBodies.length).toBe(1);
  const body = registerBodies[0] as Record<string, unknown>;
  expect(body.email).toBe("new.operator@example.test");
  expect(body.consent).toMatchObject({ version: "2026-10-07.1" });
  expect(typeof (body.consent as Record<string, unknown>).accepted_at).toBe("string");
});

test("unchecking the consent box withdraws consent and re-blocks submission", async ({ page }) => {
  await page.route("**/api/saas/**", (route) => fulfillJson(route, 200, {}));
  await page.goto("/");
  await page.getByRole("tab", { name: "Create account" }).click();
  await page.getByRole("checkbox").check();
  await expect(page.getByRole("button", { name: "Create account" })).toBeEnabled();
  await page.getByRole("checkbox").uncheck();
  await expect(page.getByRole("button", { name: "Create account" })).toBeDisabled();
});

test("forgot-password shows the backend's identical-acceptance message without revealing accounts", async ({ page }) => {
  const requestBodies: Array<Record<string, unknown>> = [];
  await page.route("**/api/saas/password-reset/**", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === "/api/saas/password-reset/request" && route.request().method() === "POST") {
      requestBodies.push(route.request().postDataJSON() as Record<string, unknown>);
      await fulfillJson(route, 202, {
        status: "accepted",
        message:
          "If that email belongs to an account here, a reset link is on its way. The link expires in 30 minutes.",
        expires_minutes: 30,
      });
      return;
    }
    await fulfillJson(route, 200, {});
  });

  await page.goto("/forgot-password");
  await page.getByLabel("Email").fill("someone@example.test");
  await page.getByRole("button", { name: /Send reset link/i }).click();

  await expect(page.getByText("If that email belongs to an account here")).toBeVisible();
  await expect(page.getByText(/30 minutes/).first()).toBeVisible();
  expect(requestBodies).toHaveLength(1);
  expect(requestBodies[0]).toEqual({ email: "someone@example.test" });
});

test("reset-password confirms a new password and reports an invalid token verbatim", async ({ page }) => {
  await page.route("**/api/saas/password-reset/**", async (route) => {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    if (body.token === "good-token") {
      await fulfillJson(route, 200, {
        status: "reset_complete",
        message: "Your password was reset. Every existing session was signed out; sign in again.",
      });
      return;
    }
    await fulfillJson(route, 422, {
      error: "invalid_or_expired_token",
      message: "This reset link is invalid or has expired. Request a new one.",
    });
  });

  // Valid token path.
  await page.goto("/reset-password?token=good-token");
  await page.locator('input[autocomplete="new-password"]').first().fill("correct-horse-battery-staple");
  await page.locator('input[autocomplete="new-password"]').nth(1).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Set new password" }).click();
  await expect(page.getByText("Your password was reset.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Go to sign in" })).toBeVisible();

  // Refused token path surfaces the backend's opaque message.
  await page.goto("/reset-password?token=stale-token");
  await page.locator('input[autocomplete="new-password"]').first().fill("correct-horse-battery-staple");
  await page.locator('input[autocomplete="new-password"]').nth(1).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Set new password" }).click();
  await expect(page.getByText("This reset link is invalid or has expired.")).toBeVisible();

  // Missing token path refuses to render the form at all.
  await page.goto("/reset-password");
  await expect(page.getByText("This reset link is missing its token.")).toBeVisible();
});

test("verify-email confirms a token and otherwise offers a resend", async ({ page }) => {
  const confirmTokens: string[] = [];
  const resendEmails: string[] = [];
  await page.route("**/api/saas/email-verification/**", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    const body = route.request().postDataJSON() as Record<string, unknown>;
    if (pathname === "/api/saas/email-verification/confirm") {
      confirmTokens.push(String(body.token));
      await fulfillJson(route, 200, {
        status: "verified",
        message: "Your email address is verified. You can sign in.",
      });
      return;
    }
    resendEmails.push(String(body.email));
    await fulfillJson(route, 202, {
      status: "accepted",
      message:
        "If that email belongs to an account here, a verification link is on its way. The link expires in 24 hours.",
    });
  });

  // Auto-confirm with a token in the URL.
  await page.goto("/verify-email?token=verify-token-1");
  await expect(page.getByText("Your email address is verified.")).toBeVisible();
  expect(confirmTokens).toEqual(["verify-token-1"]);

  // No token: the resend form appears and submits the email.
  await page.goto("/verify-email");
  await page.getByLabel("Email", { exact: true }).fill("operator@example.test");
  await page.getByRole("button", { name: "Send verification email" }).click();
  await expect(page.getByText("If that email belongs to an account here")).toBeVisible();
  expect(resendEmails).toEqual(["operator@example.test"]);
});
