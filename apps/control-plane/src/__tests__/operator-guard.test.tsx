// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OperatorGuard } from "../components/operator/OperatorGuard";
import { establishSession, clearSession } from "../lib/auth";

describe("OperatorGuard", () => {
  const fetchSpy = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("fetch", fetchSpy);
    clearSession("logout");
  });

  afterEach(() => {
    cleanup();
    clearSession("logout");
    vi.restoreAllMocks();
  });

  it("non-operator sees denied view and no operator fetch is made", () => {
    // Non-operator session: viewer role, platform_admin false
    establishSession({
      token: "test-token-viewer",
      user: {
        id: "usr-viewer",
        email: "viewer@example.test",
        email_verified: true,
        display_name: "Viewer User",
        status: "active",
        platform_admin: false,
        created_at: new Date().toISOString(),
        last_login_at: null,
      },
      session: {
        id: "ses-1",
        prefix: "ses_1",
        expires_at: new Date(Date.now() + 3600_000).toISOString(),
        organization_id: "org-1",
      },
    });

    const ChildComponent = () => {
      fetchSpy("/api/modules");
      return <div data-testid="protected-content">Operator Data</div>;
    };

    render(
      <OperatorGuard>
        <ChildComponent />
      </OperatorGuard>,
    );

    // Operator access denied message is visible
    expect(screen.getByText(/Operator Access Required/i)).toBeTruthy();
    // Protected child is never rendered
    expect(screen.queryByTestId("protected-content")).toBeNull();
    // No fetch was made to any operator route
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("operator user sees protected content", () => {
    // Operator session: platform_admin true
    establishSession({
      token: "test-token-admin",
      user: {
        id: "usr-admin",
        email: "admin@example.test",
        email_verified: true,
        display_name: "Admin User",
        status: "active",
        platform_admin: true,
        created_at: new Date().toISOString(),
        last_login_at: null,
      },
      session: {
        id: "ses-2",
        prefix: "ses_2",
        expires_at: new Date(Date.now() + 3600_000).toISOString(),
        organization_id: "org-1",
      },
    });

    render(
      <OperatorGuard>
        <div data-testid="protected-content">Operator Data</div>
      </OperatorGuard>,
    );

    expect(screen.getByTestId("protected-content")).toBeTruthy();
    expect(screen.queryByText(/Operator Access Required/i)).toBeNull();
  });
});
