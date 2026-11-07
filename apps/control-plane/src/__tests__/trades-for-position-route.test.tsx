// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ positionId: "position-a", getTrades: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ positionId: mocks.positionId }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: { tradeForPosition: (...args: unknown[]) => mocks.getTrades(...args) },
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
}));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));

import TradesForPositionPage from "../app/trading/trades/[positionId]/page";

const trade = (id: string) => ({ id, symbol: "SOL/USDC", side: "buy", venue: "test", qty: 1, price: 10, status: "filled", executed_at: "2026-11-03T00:00:00Z" });

describe("trades for position route", () => {
  beforeEach(() => {
    mocks.positionId = "position-a";
    mocks.getTrades.mockReset();
  });
  afterEach(() => cleanup());

  it("shows a terminal missing-position error", async () => {
    mocks.positionId = "";
    render(<TradesForPositionPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Position identifier is missing.");
    expect(screen.queryByText("Loading trades…")).toBeNull();
    expect(mocks.getTrades).not.toHaveBeenCalled();
  });

  it("ignores a late trade response from a previous route ID", async () => {
    let resolveOld!: (value: { trades: ReturnType<typeof trade>[] }) => void;
    mocks.getTrades
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce({ trades: [trade("trade-beta")] });
    const view = render(<TradesForPositionPage />);
    await waitFor(() => expect(mocks.getTrades).toHaveBeenCalledWith("position-a"));
    mocks.positionId = "position-b";
    view.rerender(<TradesForPositionPage />);
    expect(await screen.findByText("trade-beta…")).toBeTruthy();
    resolveOld({ trades: [trade("trade-alpha")] });
    await waitFor(() => expect(screen.getByText("trade-beta…")).toBeTruthy());
    expect(screen.queryByText("trade-alpha…")).toBeNull();
  });
});
