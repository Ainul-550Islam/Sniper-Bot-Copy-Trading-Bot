// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  copyStatus: vi.fn(),
  copyConfig: vi.fn(),
  copyLeaders: vi.fn(),
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: {
    copyStatus: (...args: unknown[]) => mocks.copyStatus(...args),
    copyConfig: (...args: unknown[]) => mocks.copyConfig(...args),
    copyLeaders: (...args: unknown[]) => mocks.copyLeaders(...args),
  },
  classifyTradingError: () => ({ kind: "unavailable" }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/trading/ModuleActionButton", () => ({ default: () => null }));
vi.mock("@/components/trading/SavedStrategiesPanel", () => ({ default: () => null }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import CopyPage from "../app/trading/copy/page";

function status() {
  return {
    effective_state: "enabled",
    tenant_override: null,
    runtime: { phase: "running", runtime_id: "runtime-1", generation: 1 },
    runtime_detail: null,
    entitlement: { feature: "module.copy", granted: true },
    controls: { available: false, actions: [], scope: "tenant" },
  };
}

describe("copy trading tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.copyStatus.mockReset().mockImplementation(() => Promise.resolve(status()));
    mocks.copyConfig.mockReset().mockImplementation(() => Promise.resolve({
      allocation_per_trade_sol: mocks.selectedOrganizationId === "org-a" ? 0.1 : 0.2,
      max_exposure_usd: mocks.selectedOrganizationId === "org-a" ? 25 : 50,
      dry_run: true,
    }));
    mocks.copyLeaders.mockReset().mockImplementation(() => Promise.resolve({
      items: [{
        address: mocks.selectedOrganizationId === "org-a" ? "alpha-wallet" : "beta-wallet",
        label: "tenant leader",
        status: "active",
        followed_at: "2026-10-30T00:00:00Z",
      }],
    }));
  });
  afterEach(() => cleanup());

  it("clears previous tenant data and reloads after organization selection changes", async () => {
    render(<CopyPage />);
    expect(await screen.findByText("alpha-wallet")).toBeTruthy();
    expect(screen.getByText("0.1 SOL")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());

    expect(await screen.findByText("beta-wallet")).toBeTruthy();
    expect(screen.getByText("0.2 SOL")).toBeTruthy();
    expect(screen.queryByText("alpha-wallet")).toBeNull();
    await waitFor(() => expect(mocks.copyLeaders).toHaveBeenCalledTimes(2));
  });
});
