// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ positionId: "position-a", getPosition: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ positionId: mocks.positionId }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({ tradingDetailApi: { position: (...args: unknown[]) => mocks.getPosition(...args) } }));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("@/components/trading/PositionDetail", () => ({ PositionDetail: ({ position }: { position: { id: string } }) => <div>Position data {position.id}</div> }));

import PositionDetailPage from "../app/trading/positions/[positionId]/page";

const record = (id: string) => ({ id, market: "SOL/USDC", side: "long", size: "1", entry_price: 10, status: "open" });

describe("position detail route", () => {
  beforeEach(() => {
    mocks.positionId = "position-a";
    mocks.getPosition.mockReset();
  });
  afterEach(() => cleanup());

  it("does not remain loading if the route identifier is missing", async () => {
    mocks.positionId = "";
    render(<PositionDetailPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Position identifier is missing.");
    expect(screen.queryByText("Loading position…")).toBeNull();
    expect(mocks.getPosition).not.toHaveBeenCalled();
  });

  it("ignores a late response for a previous position ID", async () => {
    let resolveOld!: (value: ReturnType<typeof record>) => void;
    mocks.getPosition
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(record("position-b"));
    const view = render(<PositionDetailPage />);
    await waitFor(() => expect(mocks.getPosition).toHaveBeenCalledWith("position-a"));
    mocks.positionId = "position-b";
    view.rerender(<PositionDetailPage />);
    expect(await screen.findByText("Position data position-b")).toBeTruthy();
    resolveOld(record("position-a"));
    await waitFor(() => expect(screen.getByText("Position data position-b")).toBeTruthy());
    expect(screen.queryByText("Position data position-a")).toBeNull();
  });
});
