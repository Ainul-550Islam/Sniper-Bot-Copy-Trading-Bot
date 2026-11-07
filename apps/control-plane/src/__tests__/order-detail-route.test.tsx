// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ orderId: "order-a", getOrder: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ orderId: mocks.orderId }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({ tradingDetailApi: { order: (...args: unknown[]) => mocks.getOrder(...args) } }));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("@/components/trading/OrderDetail", () => ({ OrderDetail: ({ order }: { order: { id: string } }) => <div>Order data {order.id}</div> }));

import OrderDetailPage from "../app/trading/orders/[orderId]/page";

const record = (id: string) => ({ id, status: "open", side: "buy", quantity: "1", price: "10" });

describe("order detail route", () => {
  beforeEach(() => {
    mocks.orderId = "order-a";
    mocks.getOrder.mockReset();
  });
  afterEach(() => cleanup());

  it("shows a terminal error instead of loading forever without an order ID", async () => {
    mocks.orderId = "";
    render(<OrderDetailPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Order identifier is missing.");
    expect(screen.queryByText("Loading order…")).toBeNull();
    expect(mocks.getOrder).not.toHaveBeenCalled();
  });

  it("ignores a late response for a previous order route", async () => {
    let resolveOld!: (value: ReturnType<typeof record>) => void;
    mocks.getOrder
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(record("order-b"));
    const view = render(<OrderDetailPage />);
    await waitFor(() => expect(mocks.getOrder).toHaveBeenCalledWith("order-a"));
    mocks.orderId = "order-b";
    view.rerender(<OrderDetailPage />);
    expect(await screen.findByText("Order data order-b")).toBeTruthy();
    resolveOld(record("order-a"));
    await waitFor(() => expect(screen.getByText("Order data order-b")).toBeTruthy());
    expect(screen.queryByText("Order data order-a")).toBeNull();
  });
});
