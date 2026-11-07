// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  orders: vi.fn(),
  cancelOrder: vi.fn(),
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
    orders: (...args: unknown[]) => mocks.orders(...args),
    cancelOrder: (...args: unknown[]) => mocks.cancelOrder(...args),
  },
  classifyTradingError: (error: unknown) => ({ kind: "error", reason: error instanceof Error ? error.message : "failed" }),
  isRetryable: () => false,
  tradingStateMessage: (state: { reason?: string }) => state.reason ?? "Loading",
}));
vi.mock("next/link", () => ({ default: ({ children, href }: { children: React.ReactNode; href: string }) => <a href={href}>{children}</a> }));

import OrderTable from "@/components/trading/OrderTable";

const order = (id: string, symbol: string) => ({
  id,
  idempotency_key: `${id}-key`,
  module: "sniper",
  side: "buy",
  symbol,
  venue: "test",
  mode: "paper",
  status: "open",
  qty: 1,
  price: 2,
  external_id: null,
  error: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  submitted_at: null,
  finished_at: null,
});
const page = (organizationId: string, item: ReturnType<typeof order>) => ({
  organization_id: organizationId,
  items: [item],
  next_cursor: null,
});

describe("order table tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.orders.mockReset();
    mocks.cancelOrder.mockReset();
  });
  afterEach(() => cleanup());

  it("clears orders and ignores a late response after switching organizations", async () => {
    let resolveOld!: (value: ReturnType<typeof page>) => void;
    mocks.orders
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(page("org-b", order("order-b", "B-COIN")));
    render(<OrderTable />);
    await waitFor(() => expect(mocks.orders).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("B-COIN")).toBeTruthy();
    await act(async () => resolveOld(page("org-a", order("order-a", "A-COIN"))));
    expect(screen.getByText("B-COIN")).toBeTruthy();
    expect(screen.queryByText("A-COIN")).toBeNull();
  });

  it("rejects rows returned for an organization other than the selected one", async () => {
    mocks.orders.mockResolvedValue(page("org-b", order("order-b", "B-COIN")));
    render(<OrderTable />);
    expect(await screen.findByText("Order response does not match the selected organization")).toBeTruthy();
    expect(screen.queryByText("B-COIN")).toBeNull();
  });
});
