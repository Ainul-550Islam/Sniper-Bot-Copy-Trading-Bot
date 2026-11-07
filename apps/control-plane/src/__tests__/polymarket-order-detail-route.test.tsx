// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ orderId: "venue-a", getOrder: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ orderId: mocks.orderId }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: { polymarketOrder: (...args: unknown[]) => mocks.getOrder(...args) },
}));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));

import PolymarketOrderDetailPage from "../app/trading/polymarket/orders/[orderId]/page";

const record = (venue_order_id: string) => ({ venue_order_id, market: "market-1", side: "buy", status: "open", price: 0.5, size: "5", created_at: "2026-11-03T00:00:00Z" });

describe("Polymarket order detail route", () => {
  beforeEach(() => {
    mocks.orderId = "venue-a";
    mocks.getOrder.mockReset();
  });
  afterEach(() => cleanup());

  it("shows a terminal missing-ID error without fetching", async () => {
    mocks.orderId = "";
    render(<PolymarketOrderDetailPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Polymarket order identifier is missing.");
    expect(screen.queryByText("Loading order…")).toBeNull();
    expect(mocks.getOrder).not.toHaveBeenCalled();
  });

  it("ignores a late response after navigating to another venue order", async () => {
    let resolveOld!: (value: ReturnType<typeof record>) => void;
    mocks.getOrder
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(record("venue-b"));
    const view = render(<PolymarketOrderDetailPage />);
    await waitFor(() => expect(mocks.getOrder).toHaveBeenCalledWith("venue-a"));
    mocks.orderId = "venue-b";
    view.rerender(<PolymarketOrderDetailPage />);
    expect(await screen.findByRole("heading", { name: "Polymarket Order: venue-b…" })).toBeTruthy();
    resolveOld(record("venue-a"));
    await waitFor(() => expect(screen.getByRole("heading", { name: "Polymarket Order: venue-b…" })).toBeTruthy());
    expect(screen.queryByText("venue-a")).toBeNull();
  });
});
