// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  status: vi.fn(),
  config: vi.fn(),
  orders: vi.fn(),
  fills: vi.fn(),
  reconciliation: vi.fn(),
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
    polymarketStatus: (...args: unknown[]) => mocks.status(...args),
    polymarketConfig: (...args: unknown[]) => mocks.config(...args),
    polymarketOrders: (...args: unknown[]) => mocks.orders(...args),
    polymarketFills: (...args: unknown[]) => mocks.fills(...args),
    polymarketReconciliation: (...args: unknown[]) => mocks.reconciliation(...args),
  },
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/trading/ModuleActionButton", () => ({ default: () => null }));
vi.mock("@/components/trading/SavedStrategiesPanel", () => ({ default: () => null }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import PolymarketPage from "../app/trading/polymarket/page";

function moduleStatus() {
  return {
    effective_state: "enabled",
    tenant_override: null,
    runtime: { phase: "running", runtime_id: "runtime-1", generation: 1 },
    runtime_detail: null,
    entitlement: { feature: "module.polymarket", granted: true },
    controls: { available: false, actions: [], scope: "tenant" },
  };
}

function configureMocks() {
  mocks.status.mockResolvedValue(moduleStatus());
  mocks.config.mockImplementation(() => Promise.resolve({
    dry_run: true,
    max_position_size_usdc: mocks.selectedOrganizationId === "org-a" ? 10 : 20,
    max_market_exposure_usdc: 100,
  }));
  mocks.orders.mockImplementation(() => Promise.resolve({ items: [{
    id: mocks.selectedOrganizationId === "org-a" ? "alpha-order" : "beta-order",
    market: "market-1", side: "BUY", status: "open", created_at: "2026-11-03T00:00:00Z",
  }] }));
  mocks.fills.mockResolvedValue({ items: [] });
  mocks.reconciliation.mockImplementation(() => Promise.resolve({ drift: mocks.selectedOrganizationId === "org-a" ? [{ id: "drift-1" }] : [] }));
}

describe("Polymarket tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    for (const fn of [mocks.status, mocks.config, mocks.orders, mocks.fills, mocks.reconciliation]) fn.mockReset();
    configureMocks();
  });
  afterEach(() => cleanup());

  it("clears and refetches tenant-specific orders and config on tenant switches", async () => {
    render(<PolymarketPage />);
    expect(await screen.findByText("alpha-order…")).toBeTruthy();
    expect(screen.getByText("$10")).toBeTruthy();
    expect(screen.getByText("1 DISCREPANCIES")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    await waitFor(() => {
      expect(screen.getByText("beta-order…")).toBeTruthy();
      expect(screen.getByText("$20")).toBeTruthy();
      expect(screen.getByText("IN SYNC (0 DRIFT)")).toBeTruthy();
    });
    expect(screen.queryByText("alpha-order…")).toBeNull();
    expect(mocks.orders).toHaveBeenCalledTimes(2);
  });

  it("treats malformed non-array collections as empty instead of crashing", async () => {
    mocks.orders.mockResolvedValue({ items: { unexpected: true } });
    mocks.fills.mockResolvedValue({ items: { unexpected: true } });
    mocks.reconciliation.mockResolvedValue({ drift: { unexpected: true } });
    render(<PolymarketPage />);
    expect(await screen.findByText(/No Polymarket orders recorded yet/)).toBeTruthy();
    expect(screen.getByText("IN SYNC (0 DRIFT)")).toBeTruthy();
  });
});
