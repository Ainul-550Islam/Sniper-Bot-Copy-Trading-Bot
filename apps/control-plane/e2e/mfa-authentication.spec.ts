import { expect, test, type Page, type Route } from "@playwright/test";

const organizationId = "2bd6a235-b1f5-4aa4-9d66-09c74ec88d61";
const user = {
  id: "2c9b7673-d034-4ccb-a591-d8a1ba48b741",
  email: "operator@example.test",
  email_verified: true,
  display_name: "Test Operator",
  status: "active",
  platform_admin: false,
  created_at: "2026-01-01T00:00:00.000Z",
  last_login_at: null,
};
const organization = {
  organization_id: organizationId,
  slug: "acme-capital",
  name: "Acme Capital",
  status: "active",
  role: "trader",
  membership_status: "active",
};

function session(expiresInMinutes = 15) {
  return {
    id: "a04591cf-0e0d-49c5-b865-a79a36326415",
    prefix: "ses_test1234",
    expires_at: new Date(Date.now() + expiresInMinutes * 60_000).toISOString(),
    organization_id: organizationId,
  };
}

async function fulfill(route: Route, status: number, body: unknown) {
  await route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

async function installApiMock(
  page: Page,
  handler: (route: Route, pathname: string) => Promise<void>,
) {
  await page.route("**/api/saas/**", async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    await handler(route, pathname);
  });
}

async function fulfillCurrentUser(route: Route) {
  await fulfill(route, 200, { user, organizations: [organization] });
}

test("MFA login challenge asks for tenant and code before creating a full session", async ({ page }) => {
  const loginBodies: Array<Record<string, unknown>> = [];
  await installApiMock(page, async (route, pathname) => {
    if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      loginBodies.push(body);
      if (loginBodies.length === 1) {
        await fulfill(route, 409, {
          error: "organization_required_for_mfa",
          reason: "select the protected organization and provide mfa_code",
        });
        return;
      }
      await fulfill(route, 200, {
        user,
        token: "opaque-test-session-token",
        session: session(60),
      });
      return;
    }
    if (pathname === "/api/saas/users/me") {
      await fulfillCurrentUser(route);
      return;
    }
    await fulfill(route, 200, {});
  });

  await page.goto("/");
  await page.getByLabel("Email").fill(user.email);
  await page.getByLabel("Password").fill("correct-password");
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect(page.getByLabel("Organization slug")).toBeVisible();
  await expect(page.getByLabel("Authenticator code")).toBeVisible();
  await page.getByLabel("Organization slug").fill(organization.slug);
  await page.getByLabel("Authenticator code").fill("123456");
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();
  expect(loginBodies).toHaveLength(2);
  expect(loginBodies[1]).toMatchObject({
    email: user.email,
    organization: organization.slug,
    mfa_code: "123456",
  });
});

test("existing member without a TOTP device gets only an enrollment session", async ({ page }) => {
  let loginAttempts = 0;
  const setupHeaders: Array<string | undefined> = [];
  await installApiMock(page, async (route, pathname) => {
    if (pathname === "/api/saas/sessions" && route.request().method() === "POST") {
      loginAttempts += 1;
      if (loginAttempts === 1) {
        await fulfill(route, 409, {
          error: "organization_required_for_mfa",
          reason: "select the protected organization and provide mfa_code",
        });
        return;
      }
      const body = route.request().postDataJSON() as Record<string, unknown>;
      expect(body.organization).toBe(organization.slug);
      expect(body.mfa_code).toBeUndefined();
      await fulfill(route, 200, {
        user,
        token: "restricted-enrollment-token",
        session: session(15),
        mfa_enrollment_required: true,
        organization_id: organizationId,
        organization_slug: organization.slug,
      });
      return;
    }
    if (pathname === "/api/saas/security/totp/setup") {
      setupHeaders.push(route.request().headers()["authorization"]);
      await fulfill(route, 200, {
        device_id: "678be77c-04a1-4c13-a098-e3d3c224c619",
        secret: "JBSWY3DPEHPK3PXP",
        otpauth_url: "otpauth://totp/SniperSuite:operator?secret=JBSWY3DPEHPK3PXP&issuer=SniperSuite",
        verified: false,
        backup_codes: null,
      });
      return;
    }
    if (pathname === "/api/saas/security/totp/verify") {
      expect(route.request().headers()["authorization"]).toBe("Bearer restricted-enrollment-token");
      expect(route.request().postDataJSON()).toMatchObject({ code: "654321" });
      await fulfill(route, 200, {
        success: true,
        device_id: "678be77c-04a1-4c13-a098-e3d3c224c619",
        verified: true,
        session_promoted: true,
      });
      return;
    }
    if (pathname === "/api/saas/users/me") {
      await fulfillCurrentUser(route);
      return;
    }
    await fulfill(route, 200, {});
  });

  await page.goto("/");
  await page.getByLabel("Email").fill(user.email);
  await page.getByLabel("Password").fill("correct-password");
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.getByLabel("Organization slug").fill(organization.slug);
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect(page).toHaveURL(/\/mfa-enrollment$/);
  await expect(page.getByText("JBSWY3DPEHPK3PXP")).toBeVisible();
  expect(setupHeaders).toEqual(["Bearer restricted-enrollment-token"]);
  await page.getByLabel("Current six-digit code").fill("654321");
  await page.getByRole("button", { name: "Verify and activate session" }).click();
  await expect(page).toHaveURL("/");
  await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();
});

test("MFA-enforced invitation acceptance enrolls, verifies, and removes the invite token from the URL", async ({ page }) => {
  let inviteBody: Record<string, unknown> | null = null;
  await installApiMock(page, async (route, pathname) => {
    if (pathname === "/api/saas/team/invites/accept" && route.request().method() === "POST") {
      inviteBody = route.request().postDataJSON() as Record<string, unknown>;
      await fulfill(route, 200, {
        user,
        token: "restricted-invite-session-token",
        session: session(15),
        organization_id: organizationId,
        organization_slug: organization.slug,
        mfa_enrollment_required: true,
        membership: {
          id: "5eb3d8a2-7007-49d9-b67d-0a1a3657627d",
          role: "trader",
          status: "active",
        },
      });
      return;
    }
    if (pathname === "/api/saas/security/totp/setup") {
      await fulfill(route, 200, {
        device_id: "d438ce1f-a487-4482-b6e1-dc8d21999d19",
        secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
        otpauth_url: "otpauth://totp/SniperSuite:operator?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=SniperSuite",
        verified: false,
        backup_codes: null,
      });
      return;
    }
    if (pathname === "/api/saas/security/totp/verify") {
      expect(route.request().headers()["authorization"]).toBe("Bearer restricted-invite-session-token");
      await fulfill(route, 200, {
        success: true,
        device_id: "d438ce1f-a487-4482-b6e1-dc8d21999d19",
        verified: true,
        session_promoted: true,
      });
      return;
    }
    if (pathname === "/api/saas/users/me") {
      await fulfillCurrentUser(route);
      return;
    }
    await fulfill(route, 200, {});
  });

  await page.goto("/accept-invite?token=one-time-invite-token");
  await expect(page).toHaveURL(/\/accept-invite$/);
  await page.getByLabel("Password", { exact: true }).fill("correct-password-12");
  await page.getByLabel("Confirm password").fill("correct-password-12");
  await page.getByRole("button", { name: "Accept invitation" }).click();

  await expect(page.getByText("This organization requires multi-factor authentication.")).toBeVisible();
  await expect(page.getByText("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")).toBeVisible();
  expect(inviteBody).toMatchObject({ token: "one-time-invite-token" });
  await page.getByLabel("Current six-digit authenticator code").fill("123456");
  await page.getByRole("button", { name: "Verify authenticator and activate session" }).click();
  await expect(page).toHaveURL("/");
  await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();
});
