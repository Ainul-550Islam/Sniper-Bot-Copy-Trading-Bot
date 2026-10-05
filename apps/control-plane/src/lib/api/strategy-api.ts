/**
 * Strategy Management API Client (SECOND.md §76).
 *
 * Fully typed client for tenant-scoped strategy CRUD, validation,
 * parameter configuration, and lifecycle archiving.
 */

import { tenantRequest } from "../customer-trading-api";

export type StrategyStatus = "draft" | "active" | "paused" | "archived";
export type StrategyModule = "sniper" | "copy" | "polymarket";

export interface SniperParams {
  target_tokens: string[];
  max_buy_sol: number;
  slippage_bps: number;
  anti_rug_min_liquidity_usd: number;
  auto_sell_take_profit_pct: number;
  auto_sell_stop_loss_pct: number;
  mev_protection_tip_lamports: number;
}

export interface CopyParams {
  lead_wallets: string[];
  copy_ratio_pct: number;
  max_trade_sol: number;
  follow_sells: boolean;
  min_lead_balance_sol: number;
}

export interface PolymarketParams {
  market_slugs: string[];
  max_position_usd: number;
  spread_threshold_bps: number;
  clob_order_type: string;
}

export interface StrategyRecord {
  id: string;
  organization_id: string;
  module_family: StrategyModule;
  name: string;
  description: string;
  status: StrategyStatus;
  version: number;
  parameters: SniperParams | CopyParams | PolymarketParams | Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface StrategiesResponse {
  organization_id: string;
  items: StrategyRecord[];
  count: number;
}

export interface CreateStrategyInput {
  module_family: StrategyModule;
  name: string;
  description?: string;
  parameters: Record<string, unknown>;
}

export interface UpdateStrategyInput {
  name?: string;
  description?: string;
  status?: StrategyStatus;
  parameters?: Record<string, unknown>;
}

/**
 * Fetch all strategies owned by the tenant.
 */
export async function listStrategies(module?: string): Promise<StrategyRecord[]> {
  const query = module ? `?module=${encodeURIComponent(module)}` : "";
  const res = await tenantRequest<StrategiesResponse>(`/api/tenant/strategies${query}`);
  return res.items;
}

/**
 * Fetch single strategy by unique ID.
 */
export async function getStrategy(id: string): Promise<StrategyRecord> {
  return tenantRequest<StrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`);
}

/**
 * Create a new tenant trading strategy.
 */
export async function createStrategy(input: CreateStrategyInput): Promise<StrategyRecord> {
  return tenantRequest<StrategyRecord>("/api/tenant/strategies", {
    method: "POST",
    body: input,
  });
}

/**
 * Update an existing strategy and increment its version.
 */
export async function updateStrategy(id: string, input: UpdateStrategyInput): Promise<StrategyRecord> {
  return tenantRequest<StrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: input,
  });
}

/**
 * Archive a strategy.
 */
export async function archiveStrategy(id: string): Promise<{ success: boolean; id: string; status: string }> {
  return tenantRequest<{ success: boolean; id: string; status: string }>(
    `/api/tenant/strategies/${encodeURIComponent(id)}`,
    {
      method: "DELETE",
    },
  );
}
