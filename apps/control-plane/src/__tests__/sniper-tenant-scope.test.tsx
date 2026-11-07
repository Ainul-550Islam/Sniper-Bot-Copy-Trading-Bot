// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  status: vi.fn(),
  config: vi.fn(),
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
    sniperStatus: (...args: unknown[]) => mocks.status(...args),
    sniperConfig: (...args: unknown[]) => mocks.config(...args),
  },
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/trading/ModuleActionButton", () => ({ default: () => null }));
vi.mock("@/components/trading/SavedStrategiesPanel", () => ({ default: () => null }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import SniperPage from "../app/trading/sniper/page";

function moduleStatus() {
  return {
    effective_state: "enabled",
    tenant_override: null,
    runtime: { phase: "running", runtime_id: "runtime-1", generation: 1 },
    runtime_detail: null,
    entitlement: { feature: "module.sniper", granted: true },
    controls: { available: false, actions: [], scope: "tenant" },
  };
}
const configFor = (amount: number) => ({
  dry_run: true, dex_routing: "raydium", priority_fee_lamports: 1000,
  entry_amount_sol: amount, min_liquidity_sol: 1, max_slippage_bps: 100,
  take_profit_pct: 10, stop_loss_pct: 5,
});

describe("sniper tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.status.mockReset().mockResolvedValue(moduleStatus());
    mocks.config.mockReset().mockImplementation(() => Promise.resolve(configFor(mocks.selectedOrganizationId === "org-a" ? 0.1 : 0.2)));
  });
  afterEach(() => cleanup());

  it("clears and reloads configuration when the selected tenant changes", async () => {
    render(<SniperPage />);
    expect(await screen.findByText("0.1 SOL")).toBeTruthy();
    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());

    await waitFor(() => expect(screen.getByText("0.2 SOL")).toBeTruthy());
    expect(screen.queryByText("0.1 SOL")).toBeNull();
    expect(mocks.status).toHaveBeenCalledTimes(2);
    expect(mocks.config).toHaveBeenCalledTimes(2);
  });
});
