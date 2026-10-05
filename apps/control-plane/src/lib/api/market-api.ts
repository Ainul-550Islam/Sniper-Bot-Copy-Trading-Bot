/**
 * Market Discovery & Screener API Client (SECOND.md §78).
 *
 * Fully typed client for live DEX pairs, bonding curves, CLOB books,
 * and prediction event markets.
 */

import { tenantRequest } from "../customer-trading-api";

export interface MarketTicker {
  id: string;
  symbol: string;
  name: string;
  venue: "raydium" | "pumpfun" | "orca" | "polymarket" | string;
  base_asset: string;
  quote_asset: string;
  price_usd: number;
  change_24h_pct: number;
  volume_24h_usd: number;
  liquidity_usd: number;
  is_active: boolean;
  compatible_modules: string[];
}

export interface MarketsResponse {
  organization_id: string;
  items: MarketTicker[];
  count: number;
}

/**
 * Fetch all discovered DEX/CLOB markets.
 */
export async function listMarkets(): Promise<MarketTicker[]> {
  const res = await tenantRequest<MarketsResponse>("/api/tenant/markets");
  return res.items;
}

/**
 * Fetch single market details by identifier.
 */
export async function getMarket(id: string): Promise<MarketTicker> {
  return tenantRequest<MarketTicker>(`/api/tenant/markets/${encodeURIComponent(id)}`);
}
