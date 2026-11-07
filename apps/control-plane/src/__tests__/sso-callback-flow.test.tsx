// @vitest-environment jsdom
import { StrictMode } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  replace: vi.fn(),
  completeSsoLogin: vi.fn(),
  refresh: vi.fn(),
  establishSession: vi.fn(),
  selectOrganization: vi.fn(),
  clearSession: vi.fn(),
  getSnapshot: vi.fn(),
}));

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: mocks.replace }) }));
vi.mock("@/lib/auth", () => ({
  clearSession: mocks.clearSession,
  establishSession: mocks.establishSession,
  refresh: mocks.refresh,
  selectOrganization: mocks.selectOrganization,
  sessionStore: { getSnapshot: mocks.getSnapshot },
}));
vi.mock("@/lib/api", () => ({
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : "Unknown error",
}));
vi.mock("@/lib/api/sso-api", () => ({
  completeSsoLogin: (...args: unknown[]) => mocks.completeSsoLogin(...args),
  PENDING_SSO_STATE_KEY: "sniper.sso.pending-state",
}));

import SsoCallbackPage from "@/app/sso/callback/page";

const orgId = "org-sso-fixture";
const loginResponse = {
  user: { id: "user-fixture" },
  token: "one-time-session-token",
  session: {
    id: "session-fixture",
    prefix: "ses_fixture",
    expires_at: "2030-01-01T00:00:00Z",
    organization_id: orgId,
  },
  organization_id: orgId,
  mfa_enrollment_required: false,
};

describe("SSO browser callback", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/sso/callback?state=state-fixture&code=code-fixture");
    window.sessionStorage.setItem("sniper.sso.pending-state", "state-fixture");
    mocks.replace.mockReset();
    mocks.completeSsoLogin.mockReset().mockResolvedValue(loginResponse);
    mocks.refresh.mockReset().mockResolvedValue(true);
    mocks.establishSession.mockReset();
    mocks.selectOrganization.mockReset();
    mocks.clearSession.mockReset();
    mocks.getSnapshot.mockReset().mockReturnValue({ organizations: [{ organization_id: orgId }] });
  });

  afterEach(() => cleanup());

  it("exchanges a matching state once in Strict Mode and hydrates the tenant session", async () => {
    render(<StrictMode><SsoCallbackPage /></StrictMode>);
    expect(await screen.findByText("Completing your sign-in…")).toBeTruthy();
    await vi.waitFor(() => expect(mocks.replace).toHaveBeenCalledWith("/"));

    expect(mocks.completeSsoLogin).toHaveBeenCalledTimes(1);
    expect(mocks.completeSsoLogin).toHaveBeenCalledWith("state-fixture", "code-fixture");
    expect(mocks.establishSession).toHaveBeenCalledWith(loginResponse);
    expect(mocks.refresh).toHaveBeenCalledTimes(1);
    expect(mocks.selectOrganization).toHaveBeenCalledWith(orgId);
    expect(window.location.search).toBe("");
    expect(window.sessionStorage.getItem("sniper.sso.pending-state")).toBeNull();
  });

  it("rejects a mismatched state without exchanging the authorization code", async () => {
    window.history.replaceState({}, "", "/sso/callback?state=attacker-state&code=code-fixture");
    render(<SsoCallbackPage />);

    expect(await screen.findByText("The SSO sign-in state was missing or did not match. Please try again.")).toBeTruthy();
    expect(mocks.completeSsoLogin).not.toHaveBeenCalled();
    expect(mocks.establishSession).not.toHaveBeenCalled();
    expect(window.sessionStorage.getItem("sniper.sso.pending-state")).toBeNull();
  });
});
