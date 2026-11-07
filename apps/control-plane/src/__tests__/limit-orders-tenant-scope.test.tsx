// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  list: vi.fn(),
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      selectedOrganizationId: mocks.selectedOrganizationId,
      organizations: [
        { organization_id: "org-a", role: "owner" },
        { organization_id: "org-b", role: "viewer" },
      ],
    }),
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api/limit-orders-api", () => ({
  limitOrdersApi: { list: (...args: unknown[]) => mocks.list(...args), create: vi.fn(), cancel: vi.fn() },
}));
vi.mock("@/lib/customer-trading-api", () => ({
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/limit-orders/OrderForm", () => ({
  OrderForm: ({ canManage }: { canManage: boolean }) => <div>Can manage limits: {String(canManage)}</div>,
}));
vi.mock("@/components/limit-orders/OrderTable", () => ({
  OrderTable: ({ orders }: { orders: Array<{ id: string }> }) => <div>{orders.map((order) => <span key={order.id}>{order.id}</span>)}</div>,
}));
vi.mock("@/components/limit-orders/CancelConfirm", () => ({ CancelConfirm: () => null }));

import LimitOrdersPage from "../app/trading/limit-orders/page";

const ordersFor = (id: string) => [{
  id,
  mint: `mint-${id}`,
  side: "buy",
  price: 1,
  quantity: 1,
  filled_quantity: 0,
  status: "open",
  created_at: "2026-10-30T00:00:00Z",
}];

describe("limit orders tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.list.mockReset().mockImplementation(() => Promise.resolve({
      orders: ordersFor(mocks.selectedOrganizationId === "org-a" ? "order-alpha" : "order-beta"),
      next_before: null,
    }));
  });
  afterEach(() => cleanup());

  it("clears old orders and updates create permissions when the tenant changes", async () => {
    render(<LimitOrdersPage />);
    expect(await screen.findByText("order-alpha")).toBeTruthy();
    expect(screen.getByText("Can manage limits: true")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    await waitFor(() => {
      expect(screen.getByText("order-beta")).toBeTruthy();
      expect(screen.getByText("Can manage limits: false")).toBeTruthy();
    });
    expect(screen.queryByText("order-alpha")).toBeNull();
    expect(mocks.list).toHaveBeenCalledTimes(2);
  });
});
