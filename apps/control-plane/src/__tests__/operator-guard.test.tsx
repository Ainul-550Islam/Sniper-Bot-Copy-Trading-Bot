// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OperatorGuard } from "../components/operator/OperatorGuard";
import { clearSession, establishSession } from "../lib/auth";
import {
  clearOperatorKey,
  getOperatorKey,
  setOperatorKey,
} from "../lib/operator-session";

const OPERATOR_KEY = "operator-key-material-0123456789abcdef";

type SessionUser = {
  id: string;
  platform_admin: boolean;
};

function signIn(user: SessionUser): void {
  establishSession({
    token: `token-${user.id}`,
    user: {
      id: user.id,
      email: `${user.id}@example.test`,
      email_verified: true,
      display_name: user.id,
      status: "active",
      platform_admin: user.platform_admin,
      created_at: new Date().toISOString(),
      last_login_at: null,
    },
    session: {
      id: "ses-1",
      prefix: "ses_1",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
      organization_id: "org-1",
    },
  });
}

describe("OperatorGuard", () => {
  const fetchSpy = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("fetch", fetchSpy);
    clearSession("logout");
    clearOperatorKey();
    fetchSpy.mockClear();
  });

  afterEach(() => {
    cleanup();
    clearSession("logout");
    clearOperatorKey();
    vi.restoreAllMocks();
  });

  it("a tenant owner who is not a platform admin is denied and no request fires", () => {
    // The regression this locks down: the owner/admin/operator tenant roles
    // used to map onto the operator console, which then 401'd on every call.

    signIn({ id: "usr-owner", platform_admin: false });

    const ChildComponent = () => {
      fetchSpy("/api/modules");
      return <div data-testid="protected-content">Operator Data</div>;
    };

    render(
      <OperatorGuard>
        <ChildComponent />
      </OperatorGuard>,
    );

    expect(screen.getByText(/Operator Access Required/i)).toBeTruthy();
    expect(screen.queryByTestId("protected-content")).toBeNull();
    // No unlock prompt either: this user has no business seeing one.
    expect(screen.queryByLabelText(/Operator API key/i)).toBeNull();
    // And not a single operator request was attempted.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("a tenant admin / operator who is not a platform admin is denied too", () => {
    for (const role of ["admin", "operator"]) {
      signIn({ id: `usr-${role}`, platform_admin: false });
      const { unmount } = render(
        <OperatorGuard>
          <div data-testid="protected-content">Operator Data</div>
        </OperatorGuard>,
      );
      expect(screen.getByText(/Operator Access Required/i)).toBeTruthy();
      expect(screen.queryByTestId("protected-content")).toBeNull();
      expect(fetchSpy).not.toHaveBeenCalled();
      unmount();
    }
  });

  it("a platform admin without a key sees the unlock prompt and fires no request", () => {
    signIn({ id: "usr-admin", platform_admin: true });

    const ChildComponent = () => {
      fetchSpy("/api/modules");
      return <div data-testid="protected-content">Operator Data</div>;
    };

    render(
      <OperatorGuard>
        <ChildComponent />
      </OperatorGuard>,
    );

    expect(screen.getByText(/Operator Console Locked/i)).toBeTruthy();
    const prompt = screen.getByLabelText(/Operator API key/i) as HTMLInputElement;
    expect(prompt.type).toBe("password");
    expect(screen.queryByTestId("protected-content")).toBeNull();
    // Authority comes from the key, so nothing is requested before it exists.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("a platform admin with a key renders children and can lock again", () => {
    signIn({ id: "usr-admin", platform_admin: true });
    setOperatorKey(OPERATOR_KEY);

    render(
      <OperatorGuard>
        <div data-testid="protected-content">Operator Data</div>
      </OperatorGuard>,
    );

    expect(screen.getByTestId("protected-content")).toBeTruthy();
    expect(screen.queryByText(/Operator Access Required/i)).toBeNull();
    expect(screen.queryByText(/Operator Console Locked/i)).toBeNull();

    const lock = screen.getByRole("button", { name: "Lock" });
    lock.click();
    expect(getOperatorKey()).toBeNull();
  });

  it("returns to the unlock prompt when a 401 clears the key", () => {
    signIn({ id: "usr-admin", platform_admin: true });
    setOperatorKey(OPERATOR_KEY);

    const { rerender } = render(
      <OperatorGuard>
        <div data-testid="protected-content">Operator Data</div>
      </OperatorGuard>,
    );
    expect(screen.getByTestId("protected-content")).toBeTruthy();

    // This is what lib/api/operator-api.ts does when the backend answers 401.
    clearOperatorKey("invalid or missing x-api-key");
    rerender(
      <OperatorGuard>
        <div data-testid="protected-content">Operator Data</div>
      </OperatorGuard>,
    );

    expect(screen.getByText(/Operator Console Locked/i)).toBeTruthy();
    expect(screen.getByText(/invalid or missing x-api-key/i)).toBeTruthy();
    expect(screen.queryByTestId("protected-content")).toBeNull();
  });
});
