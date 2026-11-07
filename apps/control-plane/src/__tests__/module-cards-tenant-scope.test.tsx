// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  sniperStatus: vi.fn(),
  copyStatus: vi.fn(),
  polymarketStatus: vi.fn(),
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
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: {
    sniperStatus: () => mocks.sniperStatus(),
    copyStatus: () => mocks.copyStatus(),
    polymarketStatus: () => mocks.polymarketStatus(),
  },
  classifyTradingError: (error: unknown) => ({ kind: "error", reason: error instanceof Error ? error.message : "failed" }),
  isRetryable: () => false,
  tradingStateMessage: (state: { reason?: string }) => state.reason ?? "Loading",
}));
vi.mock("@/components/trading/ModuleActionButton", () => ({ default: () => null }));

import ModuleCards from "@/components/trading/ModuleCards";

const status = (organizationId: string, module: string) => ({
  organization_id: organizationId,
  module,
  entitlement: { feature: module, granted: true },
  tenant_override: null,
  effective_state: "enabled",
  runtime: {
    runtime_id: `${organizationId}-${module}`,
    generation: "generation-1234",
    phase: `${module}-phase-${organizationId}`,
    since: "2026-01-01T00:00:00Z",
  },
  runtime_detail: null,
  controls: { available: false, actions: [], scope: "tenant" },
});

describe("module cards tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.sniperStatus.mockReset().mockResolvedValue(status("org-a", "sniper"));
    mocks.copyStatus.mockReset().mockResolvedValue(status("org-a", "copy"));
    mocks.polymarketStatus.mockReset().mockResolvedValue(status("org-a", "polymarket"));
  });
  afterEach(() => cleanup());

  it("clears previous module status and ignores a late response after switching organizations", async () => {
    let resolveOldSniper!: (value: ReturnType<typeof status>) => void;
    mocks.sniperStatus
      .mockReturnValueOnce(new Promise((resolve) => { resolveOldSniper = resolve; }))
      .mockResolvedValueOnce(status("org-b", "sniper"));
    mocks.copyStatus.mockResolvedValueOnce(status("org-a", "copy")).mockResolvedValueOnce(status("org-b", "copy"));
    mocks.polymarketStatus.mockResolvedValueOnce(status("org-a", "polymarket")).mockResolvedValueOnce(status("org-b", "polymarket"));

    render(<ModuleCards />);
    await screen.findByText("copy-phase-org-a");
    await screen.findByText("polymarket-phase-org-a");
    await waitFor(() => expect(mocks.sniperStatus).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(screen.queryByText("copy-phase-org-a")).toBeNull();
    expect(await screen.findByText("sniper-phase-org-b")).toBeTruthy();
    expect(screen.getByText("copy-phase-org-b")).toBeTruthy();
    expect(screen.getByText("polymarket-phase-org-b")).toBeTruthy();

    await act(async () => resolveOldSniper(status("org-a", "sniper")));
    expect(screen.getByText("sniper-phase-org-b")).toBeTruthy();
    expect(screen.queryByText("sniper-phase-org-a")).toBeNull();
  });

  it("rejects module status responses for a different organization", async () => {
    mocks.sniperStatus.mockResolvedValue(status("org-b", "sniper"));
    render(<ModuleCards />);
    expect(await screen.findByText("Module status response does not match the selected organization")).toBeTruthy();
    expect(screen.queryByText("sniper-phase-org-b")).toBeNull();
  });
});
