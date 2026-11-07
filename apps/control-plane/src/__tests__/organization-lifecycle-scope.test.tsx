// @vitest-environment jsdom
import type { ReactNode } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ get: vi.fn() }));
const session = vi.hoisted(() => ({
  selectedOrganizationId: "org-b",
  organizations: [
    { organization_id: "org-a", slug: "alpha", name: "Alpha", role: "owner" },
    { organization_id: "org-b", slug: "beta", name: "Beta", role: "viewer" },
  ],
  listeners: [] as Array<() => void>,
}));

vi.mock("@/lib/api/org-lifecycle-api", () => ({
  orgLifecycleApi: { get: mocks.get },
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      selectedOrganizationId: session.selectedOrganizationId,
      organizations: session.organizations,
    }),
    subscribe: (listener: () => void) => {
      session.listeners.push(listener);
      return () => {
        session.listeners = session.listeners.filter((current) => current !== listener);
      };
    },
  },
}));
vi.mock("@/lib/api", () => ({
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: ReactNode }) => <main>{children}</main>,
}));
vi.mock("@/components/settings/OrgLifecyclePanel", () => ({
  OrgLifecyclePanel: ({
    organizationId,
    organizationSlug,
    canManage,
  }: {
    organizationId: string;
    organizationSlug: string;
    canManage: boolean;
  }) => (
    <div>Organization {organizationId} ({organizationSlug}); manage={String(canManage)}</div>
  ),
}));

import OrganizationSettingsPage from "@/app/settings/organization/page";

describe("organization lifecycle selected-tenant scope", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    session.selectedOrganizationId = "org-b";
    session.listeners = [];
    mocks.get.mockImplementation(async (organizationId: string) => ({
      organization_id: organizationId,
      lifecycle_status: "active",
      state: "active",
      reason: null,
      suspended_at: null,
      suspended_by: null,
      closed_at: null,
      updated_at: "2026-10-30T00:00:00Z",
    }));
  });

  afterEach(() => cleanup());

  it("loads and authorizes the selected organization, then follows tenant switching", async () => {
    render(<OrganizationSettingsPage />);

    expect(await screen.findByText("Organization org-b (beta); manage=false")).toBeTruthy();
    expect(mocks.get).toHaveBeenCalledWith("org-b");

    act(() => {
      session.selectedOrganizationId = "org-a";
      for (const listener of session.listeners) listener();
    });

    expect(await screen.findByText("Organization org-a (alpha); manage=true")).toBeTruthy();
    expect(mocks.get).toHaveBeenLastCalledWith("org-a");
  });
});
