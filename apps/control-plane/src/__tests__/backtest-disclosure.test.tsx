// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import DataSourceBadge from "@/components/backtest/DataSourceBadge";
import type { BacktestRecord } from "@/lib/api/backtest-api";
import BacktestsPage from "@/app/backtests/page";
import BacktestDetailPage from "@/app/backtests/[runId]/page";

const mocks = vi.hoisted(() => ({
  listBacktests: vi.fn(),
  listStrategies: vi.fn(),
  getBacktest: vi.fn(),
  useParams: vi.fn(),
}));
vi.mock("@/lib/api/backtest-api", () => ({
  listBacktests: mocks.listBacktests,
  getBacktest: mocks.getBacktest,
}));
vi.mock("@/lib/api/strategy-api", () => ({
  listStrategies: mocks.listStrategies,
}));
vi.mock("next/navigation", () => ({ useParams: mocks.useParams }));
vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}));
vi.mock("@/components/backtest/BacktestRunner", () => ({
  default: () => <div />,
}));
vi.mock("@/components/backtest/BacktestMetrics", () => ({
  default: () => <div>Metrics</div>,
}));
vi.mock("@/components/common/ErrorState", () => ({
  ErrorState: () => <div>Error</div>,
}));

const run: BacktestRecord = {
  id: "run-1",
  organization_id: "org-1",
  strategy_id: "strategy-1",
  strategy_name: "Momentum",
  module: "sniper",
  venue: "raydium",
  period_start: "2026-01-01T00:00:00.000Z",
  period_end: "2026-01-02T00:00:00.000Z",
  initial_balance_usd: 1000,
  final_balance_usd: null,
  net_pnl_usd: null,
  net_roi_pct: null,
  max_drawdown_pct: null,
  total_trades: null,
  win_rate_pct: null,
  sharpe_ratio: null,
  fee_rate_bps: 25,
  slippage_bps: 10,
  status: "queued",
  created_at: "2026-01-01T00:00:00.000Z",
  completed_at: null,
  error: null,
  data_source: "synthetic",
  authoritative: false,
  blocked_reason: null,
  dataset_id: null,
  assumptions: null,
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("backtest data provenance disclosure", () => {
  it.each([
    ["synthetic", false, "Simulated data; Not authoritative"],
    ["synthetic", true, "Simulated data; Authoritative"],
    ["recorded", false, "Recorded market data; Not authoritative"],
    ["recorded", true, "Recorded market data; Authoritative"],
    [null, false, "Data source unavailable; Not authoritative"],
    [null, true, "Data source unavailable; Authoritative"],
  ] as const)("labels source=%s authoritative=%s accessibly", (dataSource, authoritative, label) => {
    render(<DataSourceBadge dataSource={dataSource} authoritative={authoritative} />);
    expect(screen.getByRole("status", { name: label })).toBeTruthy();
  });

  it("renders the same provenance disclosure on list and detail pages", async () => {
    mocks.listBacktests.mockResolvedValue([run]);
    mocks.listStrategies.mockResolvedValue([]);
    mocks.getBacktest.mockResolvedValue(run);
    mocks.useParams.mockReturnValue({ runId: "run-1" });

    const { rerender } = render(<BacktestsPage />);
    expect(await screen.findByRole("status", {
      name: "Simulated data; Not authoritative",
    })).toBeTruthy();

    rerender(<BacktestDetailPage />);
    expect(await screen.findByRole("status", {
      name: "Simulated data; Not authoritative",
    })).toBeTruthy();
    expect(screen.getByText(/A run is authoritative only when the server reports recorded market data/i)).toBeTruthy();
  });
});
