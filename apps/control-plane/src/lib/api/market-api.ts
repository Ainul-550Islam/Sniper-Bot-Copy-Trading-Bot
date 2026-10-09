/**
 * Market Discovery & Screener API client (GAP MAP v2, Part 5).
 *
 * Typed client for `GET /api/tenant/markets` and `GET /api/tenant/markets/:id`,
 * served by the tenant trading data plane. The interface mirrors the Rust
 * wire contract EXACTLY (bot_core::market_data::MarketTicker serialized with
 * serde snake_case): integer cents and basis points on the wire — the float
 * dollars/percent conversions happen in the display helpers below, in one
 * place, and only for presentation.
 *
 * Honesty rules carried over from the data plane:
 * - An empty feed is a 503 `market_data_unavailable`, surfaced as an ApiError
 *   whose reason carries the backend's human detail; the UI never invents
 *   tickers to fill the table.
 * - The list response includes `feeds` health and a `from_cache` flag; the
 *   page shows them so a stale or degraded feed is visible to the operator.
 */

import { tenantRequest } from "../customer-trading-api";

/**
 * One market row. Field names and units are the server's wire contract:
 * prices/volumes/liquidity in integer USD cents, 24h change in basis points.
 */
export interface MarketTicker {
  id: string;
  symbol: string;
  name: string;
  /** snake_case wire form of the Rust `Venue` enum. */
  venue: string;
  base_asset: string;
  quote_asset: string;
  price_usd_cents: number;
  change_24h_bps: number;
  volume_24h_usd_cents: number;
  liquidity_usd_cents: number;
  is_active: boolean;
  /** snake_case module ids (`sniper`, `copy`, `polymarket`, …). */
  compatible_modules: string[];
  updated_at: string;
}

/** Health of one upstream market-data feed (`FeedStatus` on the server). */
export interface FeedStatus {
  name: string;
  ok: boolean;
  /** Human-readable error when `ok` is false, empty otherwise. */
  detail: string;
  fetched_at: string;
  tickers: number;
}

/** `GET /api/tenant/markets` success body. */
export interface MarketsResponse {
  items: MarketTicker[];
  count: number;
  fetched_at: string;
  from_cache: boolean;
  feeds: FeedStatus[];
}

/**
 * Fetch discovered markets plus feed health. An empty feed is a 503 from the
 * server and therefore throws an {@link ApiError} — callers render that
 * refusal, never a fabricated list.
 */
export async function listMarkets(): Promise<MarketsResponse> {
  return tenantRequest<MarketsResponse>("/api/tenant/markets");
}

/** Fetch a single market by id; unknown ids are a 404 `market_not_found`. */
export async function getMarket(id: string): Promise<MarketTicker> {
  return tenantRequest<MarketTicker>(`/api/tenant/markets/${encodeURIComponent(id)}`);
}

// ---------------------------------------------------------------------------
// Display helpers — the ONLY place wire units become presentation units.
// ---------------------------------------------------------------------------

/** Integer cents → US dollars (presentation only). */
export function priceUsd(ticker: MarketTicker): number {
  return ticker.price_usd_cents / 100;
}

/** Basis points → percent (presentation only). */
export function changePct(ticker: MarketTicker): number {
  return ticker.change_24h_bps / 100;
}

/** Human label for the snake_case venue wire value; unknown values pass through. */
export function venueLabel(venue: string): string {
  switch (venue) {
    case "pump_fun":
      return "Pump.fun";
    case "pump_swap":
      return "PumpSwap";
    case "raydium_amm_v4":
      return "Raydium AMM v4";
    case "raydium_clmm":
      return "Raydium CLMM";
    case "jupiter":
      return "Jupiter";
    case "polymarket_clob":
      return "Polymarket CLOB";
    case "paper":
      return "Paper";
    default:
      return venue;
  }
}
