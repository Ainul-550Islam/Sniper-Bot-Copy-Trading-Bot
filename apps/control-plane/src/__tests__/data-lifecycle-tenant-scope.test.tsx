// @vitest-environment jsdom
import type { ReactNode } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  lifecycleStatus: vi.fn(),
  disconnect: vi.fn(),
  revokeCredentials: vi.fn(),
  revokeSessions: vi.fn(),
  scheduleRetention: vi.fn(),
  exportGet: vi.fn(),
}));
const session = vi.hoisted(() => ({
  selectedOrganizationId: "org-b",
  listeners: [] as Array<() => void>,
}));

vi.mock("@/lib/api", () => ({
  request: mocks.request,
  exportsApi: { get: mocks.exportGet },
  ApiError: class ApiError extends Error {},
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({ selectedOrganizationId: session.selectedOrganizationId }),
    subscribe: (listener: () => void) => {
      session.listeners.push(listener);
      return () => {
        session.listeners = session.listeners.filter((current) => current !== listener);
      };
    },
  },
}));
vi.mock("@/lib/commercial", () => ({
  commercial: { lifecycleStatus: mocks.lifecycleStatus },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/lib/api/lifecycle-actions-api", () => ({
  lifecycleActionsApi: {
    disconnect: mocks.disconnect,
    revokeCredentials: mocks.revokeCredentials,
    revokeSessions: mocks.revokeSessions,
    scheduleRetention: mocks.scheduleRetention,
  },
}));
vi.mock("@/components/AppShell", () => ({
  AppShell: ({ children }: { children: ReactNode }) => <main>{children}</main>,
}));
vi.mock("@/components/lifecycle/LifecycleActions", () => ({
  LifecycleActions: ({
    organizationId,
    canManage,
  }: {
    organizationId: string;
    canManage: boolean;
  }) => <div>Lifecycle controls {organizationId}; manage={String(canManage)}</div>,
}));

import DataLifecyclePage from "@/app/settings/data-lifecycle/page";

describe("tenant lifecycle organization scope", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    session.selectedOrganizationId = "org-b";
    session.listeners = [];
    mocks.request.mockResolvedValue({
      organizations: [
        { organization_id: "org-a", role: "owner" },
        { organization_id: "org-b", role: "viewer" },
      ],
    });
    mocks.lifecycleStatus.mockImplementation(async (organizationId: string) => ({
      organization_id: organizationId,
      organization_status: organizationId,
      phase: "operational",
      retention_scheduled: false,
      purge_eligible_at: null,
    }));
  });

  afterEach(() => cleanup());

  it("uses the selected organization, updates on switching, and gates closure by that role", async () => {
    render(<DataLifecyclePage />);

    expect(await screen.findByRole("heading", { name: "Tenant Lifecycle State: ORG-B" })).toBeTruthy();
    expect(mocks.lifecycleStatus).toHaveBeenCalledWith("org-b");
    expect(screen.getByText("Lifecycle controls org-b; manage=false")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Request Irreversible Deprovisioning" })).toBeNull();

    act(() => {
      session.selectedOrganizationId = "org-a";
      for (const listener of session.listeners) listener();
    });

    expect(await screen.findByRole("heading", { name: "Tenant Lifecycle State: ORG-A" })).toBeTruthy();
    expect(mocks.lifecycleStatus).toHaveBeenLastCalledWith("org-a");
    expect(screen.getByText("Lifecycle controls org-a; manage=true")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Request Irreversible Deprovisioning" })).toBeTruthy();
  });
});
