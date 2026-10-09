/**
 * Backtesting Engine API Client (SECOND.md §77).
 *
 * Fully typed client for deterministic strategy simulation, historical
 * performance analytics, Sharpe ratio, and drawdown metrics.
 */

import { tenantRequest } from "../customer-trading-api";

export type BacktestStatus = "queued" | "pending" | "running" | "completed" | "failed" | "cancelled";

export interface BacktestRecord {
  id: string;
  organization_id: string;
  strategy_id: string;
  strategy_name: string;
  venue: string;
  period_start: string;
  period_end: string;
  initial_balance_usd: number;
  final_balance_usd: number | null;
  net_pnl_usd: number | null;
  net_roi_pct: number | null;
  max_drawdown_pct: number | null;
  total_trades: number | null;
  win_rate_pct: number | null;
  sharpe_ratio: number | null;
  fee_rate_bps: number;
  slippage_bps: number;
  status: BacktestStatus;
  created_at: string;
  completed_at: string | null;
  error: string | null;
}

export interface BacktestsResponse {
  organization_id: string;
  items: BacktestRecord[];
  count: number;
}

export interface BacktestRunInput {
  strategy_id: string;
  period_start: string;
  period_end: string;
  venue: string;
  initial_balance_usd: number;
  fee_rate_bps: number;
  slippage_bps: number;
}

/**
 * Fetch all historical and active backtests for the tenant.
 */
export async function listBacktests(): Promise<BacktestRecord[]> {
  const res = await tenantRequest<BacktestsResponse>("/api/tenant/backtests");
  return res.items;
}

/**
 * Fetch single backtest result by ID.
 */
export async function getBacktest(id: string): Promise<BacktestRecord> {
  return tenantRequest<BacktestRecord>(`/api/tenant/backtests/${encodeURIComponent(id)}`);
}

/**
 * Queue a new backtest simulation run.
 */
export async function runBacktest(input: BacktestRunInput): Promise<BacktestRecord> {
  return tenantRequest<BacktestRecord>("/api/tenant/backtests", {
    method: "POST",
    body: input,
  });
}
