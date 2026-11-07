// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  trades: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api", () => ({ toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: { trades: (...args: unknown[]) => mocks.trades(...args) },
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import TradesPage from "../app/trading/trades/page";

const trade = (id: string) => ({
  id,
  position_id: `position-${id}`,
  symbol: "SOL/USDC",
  side: "buy",
  venue: "test",
  qty: 1,
  price: 10,
  status: "filled",
  executed_at: "2026-11-03T00:00:00Z",
});

describe("trades tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.trades.mockReset().mockImplementation(() => Promise.resolve({
      items: [trade(mocks.selectedOrganizationId === "org-a" ? "trade-alpha" : "trade-beta")],
    }));
  });
  afterEach(() => cleanup());

  it("clears and reloads the trade book after the selected organization changes", async () => {
    render(<TradesPage />);
    expect(await screen.findByText("trade-alpha…")).toBeTruthy();
    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());

    await waitFor(() => expect(screen.getByText("trade-beta…")).toBeTruthy());
    expect(screen.queryByText("trade-alpha…")).toBeNull();
    expect(mocks.trades).toHaveBeenCalledTimes(2);
  });
});
