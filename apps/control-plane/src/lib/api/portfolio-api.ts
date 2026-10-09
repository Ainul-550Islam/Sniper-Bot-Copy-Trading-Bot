/**
 * Typed Tenant Portfolio & Exposure Client (THIRD.md §126).
 *
 * Interfaces with `/api/saas/portfolio` using exact monetary units.
 */

import { request } from "../api";

export interface AssetExposure {
  asset_symbol: string;
  amount_lamports?: number;
  amount_units: number;
  value_usd_cents: number;
  percentage_bps: number;
  venue: string;
}

export interface PortfolioSummary {
  organization_id: string;
  total_equity_usd_cents: number;
  available_cash_usd_cents: number;
  allocated_margin_usd_cents: number;
  unrealized_pnl_usd_cents: number;
  realized_pnl_30d_usd_cents: number;
  max_drawdown_bps: number;
  exposures: AssetExposure[];
  as_of: string;
  is_stale: boolean;
}

/** Fetches authoritative tenant portfolio and asset exposure state. */
export async function getPortfolioSummary(): Promise<PortfolioSummary> {
  return request<PortfolioSummary>("/api/saas/portfolio");
}
