// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  getSecurityStatus: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => mocks.snapshot,
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api/security-api", () => ({ getSecurityStatus: (...args: unknown[]) => mocks.getSecurityStatus(...args) }));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/settings/security-form", () => ({
  default: ({ ipAllowlist }: { ipAllowlist: string[] }) => <div>Allowlist: {ipAllowlist.join(",")}</div>,
}));
vi.mock("@/components/settings/CurrentSessionPanel", () => ({ default: () => <div>Session inventory</div> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));

import SecuritySettingsPage from "../app/settings/security/page";

const posture = (organizationId: string, cidr: string) => ({
  organization_id: organizationId,
  mfa_enforced: false,
  has_totp_configured: false,
  ip_allowlist: [cidr],
  ip_allowlist_count: 1,
  session_duration_hours: 12,
  require_signed_commits: false,
});

describe("security settings tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.getSecurityStatus.mockReset();
  });
  afterEach(() => cleanup());

  it("drops old security settings and ignores a late response after switching organizations", async () => {
    let resolveOld!: (value: ReturnType<typeof posture>) => void;
    mocks.getSecurityStatus
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(posture("org-b", "198.51.100.0/24"));
    render(<SecuritySettingsPage />);
    await waitFor(() => expect(mocks.getSecurityStatus).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("Allowlist: 198.51.100.0/24")).toBeTruthy();
    resolveOld(posture("org-a", "192.0.2.0/24"));
    await waitFor(() => expect(screen.getByText("Allowlist: 198.51.100.0/24")).toBeTruthy());
    expect(screen.queryByText("Allowlist: 192.0.2.0/24")).toBeNull();
  });
});
