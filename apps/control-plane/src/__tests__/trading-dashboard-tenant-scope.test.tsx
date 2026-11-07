// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  orders: vi.fn(),
  positions: vi.fn(),
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
    orders: (...args: unknown[]) => mocks.orders(...args),
    positions: (...args: unknown[]) => mocks.positions(...args),
  },
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/trading/RuntimeCard", () => ({ default: () => null }));
vi.mock("@/components/trading/PnlCard", () => ({ default: () => null }));
vi.mock("@/components/trading/ModuleCards", () => ({ default: () => null }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import TradingDashboardPage from "../app/trading/page";

describe("trading dashboard tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.orders.mockReset().mockImplementation(() => Promise.resolve({
      items: Array.from({ length: mocks.selectedOrganizationId === "org-a" ? 2 : 5 }, (_, index) => ({ id: `order-${index}` })),
    }));
    mocks.positions.mockReset().mockImplementation(() => Promise.resolve({
      items: Array.from({ length: mocks.selectedOrganizationId === "org-a" ? 1 : 3 }, () => ({ status: "open" })),
    }));
  });
  afterEach(() => cleanup());

  it("clears and reloads tenant metrics when the selected organization changes", async () => {
    const view = render(<TradingDashboardPage />);
    await waitFor(() => expect(view.container.querySelectorAll(".stat-card__value")[0]?.textContent).toBe("1"));
    expect(view.container.querySelectorAll(".stat-card__value")[1]?.textContent).toBe("2");

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    await waitFor(() => {
      const values = view.container.querySelectorAll(".stat-card__value");
      expect(values[0]?.textContent).toBe("3");
      expect(values[1]?.textContent).toBe("5");
    });
    expect(mocks.orders).toHaveBeenCalledTimes(2);
    expect(mocks.positions).toHaveBeenCalledTimes(2);
    expect(screen.queryByText("Loading…")).toBeNull();
  });
});
