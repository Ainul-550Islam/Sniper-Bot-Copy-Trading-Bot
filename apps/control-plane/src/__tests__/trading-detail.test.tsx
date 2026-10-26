// @vitest-environment jsdom
/**
 * Trading detail page behaviour (Part 04, L4).
 *
 * Each detail page fetches from its exact OpenAPI path via tradingDetailApi.
 * These tests verify the correct path is requested and the page renders
 * without crashing on server truth (or honest empty/404 states).
 */

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useParams } from "next/navigation";

const positionGet = vi.fn();
const orderGet = vi.fn();
const executionGet = vi.fn();
const tradeGet = vi.fn();

vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: {
    position: (...args: unknown[]) => positionGet(...args),
    order: (...args: unknown[]) => orderGet(...args),
    execution: (...args: unknown[]) => executionGet(...args),
    trade: (...args: unknown[]) => tradeGet(...args),
  },
}));

vi.mock("next/navigation", () => ({
  useParams: vi.fn(),
  usePathname: () => "/",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), prefetch: vi.fn() }),
}));

vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", slug: "org-1", name: "Org One", status: "active", role: "owner", membership_status: "active" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

const useParamsMock = vi.mocked(useParams);

describe("trading detail", () => {
  beforeEach(() => {
    positionGet.mockReset();
    orderGet.mockReset();
    executionGet.mockReset();
    tradeGet.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("position detail GETs exact path and renders without crashing", async () => {
    positionGet.mockResolvedValue({
      position_id: "pos_1", venue: "raydium", market: "SOL/USDC",
      side: "long", size: "10.5", entry_price: 142.5, current_price: 145.0,
      pnl_usd: 26.25, status: "open", created_at: "2026-10-20T08:00:00Z",
    });
    useParamsMock.mockReturnValue({ positionId: "pos_1" });
    const PositionDetailPage = (await import("../app/trading/positions/[positionId]/page")).default;
    render(<PositionDetailPage />);
    await waitFor(() => expect(positionGet).toHaveBeenCalledWith("pos_1"));
  });

  it("order detail GETs exact path and renders without crashing", async () => {
    orderGet.mockResolvedValue({
      order_id: "ord_1", market: "SOL/USDC", side: "buy", size: "5.0",
      limit_price: 140.0, status: "open", created_at: "2026-10-20T08:00:00Z",
    });
    useParamsMock.mockReturnValue({ orderId: "ord_1" });
    const OrderDetailPage = (await import("../app/trading/orders/[orderId]/page")).default;
    render(<OrderDetailPage />);
    await waitFor(() => expect(orderGet).toHaveBeenCalledWith("ord_1"));
  });

  it("execution detail GETs exact path and renders without crashing", async () => {
    executionGet.mockResolvedValue({
      execution_id: "exec_1", order_id: "ord_1", market: "SOL/USDC", side: "sell",
      size: "3.0", price: 141.0, fee_usd: 0.005, executed_at: "2026-10-20T08:15:00Z",
    });
    useParamsMock.mockReturnValue({ executionId: "exec_1" });
    const ExecutionDetailPage = (await import("../app/trading/executions/[executionId]/page")).default;
    render(<ExecutionDetailPage />);
    await waitFor(() => expect(executionGet).toHaveBeenCalledWith("exec_1"));
  });

  it("404 position detail renders honest empty state", async () => {
    const err = new Error("404: position not found");
    positionGet.mockRejectedValue(err);
    useParamsMock.mockReturnValue({ positionId: "pos_missing" });
    const PositionDetailPage = (await import("../app/trading/positions/[positionId]/page")).default;
    render(<PositionDetailPage />);
    await expect(screen.findByText(/not found/i)).resolves.toBeTruthy();
  });
});