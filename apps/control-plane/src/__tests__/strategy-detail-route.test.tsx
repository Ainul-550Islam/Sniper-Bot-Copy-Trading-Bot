// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  strategyId: "alpha",
  getStrategy: vi.fn(),
}));

vi.mock("next/navigation", () => ({ useParams: () => ({ strategyId: mocks.strategyId }) }));
vi.mock("@/lib/api/strategy-api", () => ({ getStrategy: (...args: unknown[]) => mocks.getStrategy(...args) }));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/strategy/StrategyVersionHistory", () => ({
  default: ({ strategy }: { strategy: { name: string } }) => <div>History for {strategy.name}</div>,
}));

import StrategyDetailPage from "../app/strategies/[strategyId]/page";

const strategy = (name: string) => ({ id: name, name, versions: [], status: "active" });

describe("strategy detail route loading", () => {
  beforeEach(() => {
    mocks.strategyId = "alpha";
    mocks.getStrategy.mockReset();
  });
  afterEach(() => cleanup());

  it("shows a terminal missing-ID error instead of loading forever", async () => {
    mocks.strategyId = "";
    render(<StrategyDetailPage />);
    expect(await screen.findByText("Strategy identifier is missing.")).toBeTruthy();
    expect(screen.queryByText("Loading strategy details...")).toBeNull();
    expect(mocks.getStrategy).not.toHaveBeenCalled();
  });

  it("ignores a late response for the previous route parameter", async () => {
    let resolveAlpha!: (value: ReturnType<typeof strategy>) => void;
    mocks.getStrategy
      .mockReturnValueOnce(new Promise((resolve) => { resolveAlpha = resolve; }))
      .mockResolvedValueOnce(strategy("beta"));
    const view = render(<StrategyDetailPage />);
    await waitFor(() => expect(mocks.getStrategy).toHaveBeenCalledWith("alpha"));

    mocks.strategyId = "beta";
    view.rerender(<StrategyDetailPage />);
    expect(await screen.findByText("History for beta")).toBeTruthy();

    resolveAlpha(strategy("alpha"));
    await waitFor(() => expect(screen.getByText("History for beta")).toBeTruthy());
    expect(screen.queryByText("History for alpha")).toBeNull();
  });
});
