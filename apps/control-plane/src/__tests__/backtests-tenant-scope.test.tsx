// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  organizationId: "org-a",
  listeners: new Set<() => void>(),
  listBacktests: vi.fn(),
  listStrategies: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api/backtest-api", () => ({ listBacktests: (...args: unknown[]) => mocks.listBacktests(...args) }));
vi.mock("@/lib/api/strategy-api", () => ({ listStrategies: (...args: unknown[]) => mocks.listStrategies(...args) }));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/backtest/backtest-runner", () => ({ default: () => <div>Backtest runner form</div> }));
vi.mock("@/components/backtest/backtest-table", () => ({
  default: ({ items }: { items: Array<{ id: string }> }) => <div>{items.map((item) => <span key={item.id}>{item.id}</span>)}</div>,
}));

import BacktestsPage from "../app/backtests/page";

const run = (id: string) => ({ id });
const strategy = (id: string) => ({ id });

describe("backtests tenant scope", () => {
  beforeEach(() => {
    mocks.organizationId = "org-a";
    mocks.listeners.clear();
    mocks.listBacktests.mockReset().mockImplementation(() => Promise.resolve([run(mocks.organizationId === "org-a" ? "run-alpha" : "run-beta")]));
    mocks.listStrategies.mockReset().mockImplementation(() => Promise.resolve([strategy(mocks.organizationId === "org-a" ? "strategy-alpha" : "strategy-beta")]));
  });
  afterEach(() => cleanup());

  it("drops old rows and ignores stale tenant responses after an organization switch", async () => {
    let resolveOldRuns!: (value: Array<{ id: string }>) => void;
    let resolveOldStrategies!: (value: Array<{ id: string }>) => void;
    mocks.listBacktests.mockReturnValueOnce(new Promise((resolve) => { resolveOldRuns = resolve; }));
    mocks.listStrategies.mockReturnValueOnce(new Promise((resolve) => { resolveOldStrategies = resolve; }));
    render(<BacktestsPage />);
    await waitFor(() => {
      expect(mocks.listBacktests).toHaveBeenCalledTimes(1);
      expect(mocks.listStrategies).toHaveBeenCalledTimes(1);
    });

    mocks.organizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    expect(await screen.findByText("run-beta")).toBeTruthy();

    resolveOldRuns([run("run-alpha")]);
    resolveOldStrategies([strategy("strategy-alpha")]);
    await waitFor(() => expect(screen.getByText("run-beta")).toBeTruthy());
    expect(screen.queryByText("run-alpha")).toBeNull();
  });
});
