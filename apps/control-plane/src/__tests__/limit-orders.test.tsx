// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tenantRequest = vi.fn();

vi.mock("@/lib/customer-trading-api", () => ({
  tenantRequest: (...args: unknown[]) => tenantRequest(...args),
  classifyTradingError: (error: unknown) => ({
    kind: "error",
    error: String(error),
    retryable: false,
  }),
  isRetryable: () => false,
  tradingStateMessage: (state: { kind: string }) => `state:${state.kind}`,
}));

vi.mock("@/components/AppShell", () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", role: "owner" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

import LimitOrdersPage from "../app/trading/limit-orders/page";
import { CancelConfirm } from "../components/limit-orders/CancelConfirm";
import { OrderForm } from "../components/limit-orders/OrderForm";
import { OrderTable } from "../components/limit-orders/OrderTable";
import { limitOrdersApi, type LimitOrder } from "../lib/api/limit-orders-api";

const order: LimitOrder = {
  id: "lo_123",
  mint: "So11111111111111111111111111111111111111112",
  side: "buy",
  trigger: "at_or_below",
  price_sol: 0.01,
  amount: 25,
  status: "active",
  created_at: "2026-10-25T08:00:00Z",
  expires_at: null,
  resolved_at: null,
  attempts: 0,
  last_error: null,
};

describe("Limit orders", () => {
  beforeEach(() => {
    tenantRequest.mockReset();
  });
  afterEach(() => cleanup());

  it("uses the exact tenant endpoints and payloads", async () => {
    tenantRequest.mockResolvedValue({});
    await limitOrdersApi.list({ status: "active", limit: 50 });
    await limitOrdersApi.create({
      mint: order.mint,
      side: "buy",
      trigger: "at_or_below",
      price_sol: 0.01,
      amount: 25,
      expires_in_secs: 86400,
    });
    await limitOrdersApi.cancel(order.id);

    expect(tenantRequest.mock.calls).toEqual([
      ["/api/tenant/limit-orders?status=active&limit=50"],
      [
        "/api/tenant/limit-orders",
        {
          method: "POST",
          body: {
            mint: order.mint,
            side: "buy",
            trigger: "at_or_below",
            price_sol: 0.01,
            amount: 25,
            expires_in_secs: 86400,
          },
        },
      ],
      ["/api/tenant/limit-orders/lo_123/cancel", { method: "POST" }],
    ]);
  });

  it("page shows cancelled status only after the server refetch returns it", async () => {
    let current = { ...order };
    tenantRequest.mockImplementation(async (url: string, options?: { method?: string }) => {
      if (url === "/api/tenant/limit-orders" && !options?.method) {
        return { organization_id: "org-1", orders: [current], next_before: null };
      }
      if (url === `/api/tenant/limit-orders/${order.id}/cancel`) {
        current = { ...current, status: "cancelled" };
        return { id: order.id, status: "cancelled" };
      }
      throw new Error(`Unexpected request: ${url}`);
    });

    render(<LimitOrdersPage />);
    expect(await screen.findByText("active")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByText("active")).toBeTruthy();
    expect(screen.queryByText("cancelled")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Confirm cancellation" }));
    expect(await screen.findByText("cancelled")).toBeTruthy();
    expect(tenantRequest.mock.calls.map(([url]) => url)).toEqual([
      "/api/tenant/limit-orders",
      `/api/tenant/limit-orders/${order.id}/cancel`,
      "/api/tenant/limit-orders",
    ]);
  });

  it("page creates an order and renders it only from the refetched list", async () => {
    let created = false;
    tenantRequest.mockImplementation(async (url: string, options?: { method?: string }) => {
      if (url === "/api/tenant/limit-orders" && !options?.method) {
        return {
          organization_id: "org-1",
          orders: created ? [order] : [],
          next_before: null,
        };
      }
      if (url === "/api/tenant/limit-orders" && options?.method === "POST") {
        created = true;
        return { id: order.id, organization_id: "org-1", status: "active" };
      }
      throw new Error(`Unexpected request: ${url}`);
    });

    render(<LimitOrdersPage />);
    expect(
      await screen.findByText("No limit orders recorded for this organization yet."),
    ).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Token mint"), { target: { value: order.mint } });
    fireEvent.change(screen.getByLabelText("Trigger price (SOL)"), { target: { value: "0.01" } });
    fireEvent.change(screen.getByLabelText("Amount"), { target: { value: "25" } });
    fireEvent.click(screen.getByRole("button", { name: "Create limit order" }));
    expect(await screen.findByText("active")).toBeTruthy();
    expect(tenantRequest.mock.calls.map(([url]) => url)).toEqual([
      "/api/tenant/limit-orders",
      "/api/tenant/limit-orders",
      "/api/tenant/limit-orders",
    ]);
    expect(tenantRequest.mock.calls[1]?.[1]).toEqual({
      method: "POST",
      body: {
        mint: order.mint,
        side: "buy",
        trigger: "at_or_below",
        price_sol: 0.01,
        amount: 25,
      },
    });
  });

  it("page shows an error state when the order list cannot load", async () => {
    tenantRequest.mockRejectedValue(new Error("storage unavailable"));
    render(<LimitOrdersPage />);
    expect(await screen.findByText("state:error")).toBeTruthy();
  });

  it("renders server status and only requests cancellation from the table", () => {
    const onRequestCancel = vi.fn();
    render(
      <OrderTable
        orders={[order]}
        canManage
        busyOrderId={null}
        onRequestCancel={onRequestCancel}
      />,
    );

    expect(screen.getByText("active")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onRequestCancel).toHaveBeenCalledWith(order);
    expect(screen.getByText("active")).toBeTruthy();
    expect(screen.queryByText("cancelled")).toBeNull();
  });

  it("submits the create form using server field names", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<OrderForm canManage busy={false} onCreate={onCreate} />);

    fireEvent.change(screen.getByLabelText("Token mint"), { target: { value: order.mint } });
    fireEvent.change(screen.getByLabelText("Side"), { target: { value: "sell" } });
    fireEvent.change(screen.getByLabelText("Trigger"), { target: { value: "at_or_above" } });
    fireEvent.change(screen.getByLabelText("Trigger price (SOL)"), { target: { value: "0.02" } });
    fireEvent.change(screen.getByLabelText("Amount"), { target: { value: "0.5" } });
    fireEvent.change(screen.getByLabelText("Expires in (seconds, optional)"), { target: { value: "3600" } });
    fireEvent.click(screen.getByRole("button", { name: "Create limit order" }));

    await waitFor(() =>
      expect(onCreate).toHaveBeenCalledWith({
        mint: order.mint,
        side: "sell",
        trigger: "at_or_above",
        price_sol: 0.02,
        amount: 0.5,
        expires_in_secs: 3600,
      }),
    );
  });

  it("refuses a sell amount above the server's position-fraction limit", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<OrderForm canManage busy={false} onCreate={onCreate} />);

    fireEvent.change(screen.getByLabelText("Token mint"), { target: { value: order.mint } });
    fireEvent.change(screen.getByLabelText("Side"), { target: { value: "sell" } });
    fireEvent.change(screen.getByLabelText("Trigger price (SOL)"), { target: { value: "0.02" } });
    fireEvent.change(screen.getByLabelText("Amount"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Create limit order" }));

    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(onCreate).not.toHaveBeenCalled();
  });

  it("requires explicit cancellation confirmation", () => {
    const onConfirm = vi.fn();
    render(<CancelConfirm order={order} busy={false} onConfirm={onConfirm} onDismiss={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Keep order" }));
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm cancellation" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables management actions for a read-only role", () => {
    render(<OrderTable orders={[order]} canManage={false} busyOrderId={null} onRequestCancel={() => {}} />);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
