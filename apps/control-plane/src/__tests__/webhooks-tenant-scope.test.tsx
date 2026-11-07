// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  listWebhooks: vi.fn(),
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
vi.mock("@/lib/api/webhook-api", () => ({ listWebhooks: () => mocks.listWebhooks() }));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/settings/webhook-form", () => ({
  default: ({ webhooks }: { webhooks: Array<{ url: string }> }) => <div>Endpoints: {webhooks.map(({ url }) => url).join(",")}</div>,
}));

import WebhooksPage from "../app/settings/webhooks/page";

const endpoint = (organizationId: string, url: string) => ({
  id: `${organizationId}-endpoint`,
  organization_id: organizationId,
  url,
  description: "",
  events: [],
  is_active: true,
  created_at: "2026-01-01T00:00:00Z",
});

describe("webhooks tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.listWebhooks.mockReset();
  });
  afterEach(() => cleanup());

  it("clears old endpoints and ignores a late list response after switching organizations", async () => {
    let resolveOld!: (value: ReturnType<typeof endpoint>[]) => void;
    mocks.listWebhooks
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce([endpoint("org-b", "https://b.example/hook")]);
    render(<WebhooksPage />);
    await waitFor(() => expect(mocks.listWebhooks).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("Endpoints: https://b.example/hook")).toBeTruthy();
    resolveOld([endpoint("org-a", "https://a.example/hook")]);
    await waitFor(() => expect(screen.getByText("Endpoints: https://b.example/hook")).toBeTruthy());
    expect(screen.queryByText("Endpoints: https://a.example/hook")).toBeNull();
  });

  it("rejects endpoint records that do not belong to the selected organization", async () => {
    mocks.listWebhooks.mockResolvedValue([endpoint("org-b", "https://b.example/hook")]);
    render(<WebhooksPage />);
    expect(await screen.findByText("Webhook endpoint response does not match the selected organization")).toBeTruthy();
    expect(screen.queryByText("Endpoints: https://b.example/hook")).toBeNull();
  });
});
