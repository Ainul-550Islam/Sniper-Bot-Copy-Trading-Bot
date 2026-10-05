/**
 * Tenant strategy API client.
 *
 * The browser-facing model intentionally keeps the names used by the control
 * plane UI (`module_family` and `parameters`). The Rust API uses its canonical
 * wire names (`module` and `config_json` on responses, `module` and `config`
 * on writes). This adapter performs that translation in one place so pages do
 * not send a shape the server cannot deserialize or render a mismatched
 * response as if it were authoritative.
 */

import { tenantRequest } from "../customer-trading-api";

export type StrategyStatus = "active" | "paused" | "archived";
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

interface ServerStrategyRecord {
  id: string;
  organization_id: string;
  name: string;
  description: string;
  module: string;
  mode: "paper" | "simulate" | "live";
  status: StrategyStatus;
  version: number;
  config_json: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface StrategiesResponse {
  organization_id: string;
  items: StrategyRecord[];
  count: number;
}

interface ServerStrategiesResponse {
  organization_id: string;
  items: ServerStrategyRecord[];
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

function normalizeModule(module: string): StrategyModule {
  if (module === "sniper" || module === "copy" || module === "polymarket") {
    return module;
  }
  throw new Error(`strategy-api: server returned unsupported module '${module}'`);
}

function fromServer(record: ServerStrategyRecord): StrategyRecord {
  return {
    id: record.id,
    organization_id: record.organization_id,
    module_family: normalizeModule(record.module),
    name: record.name,
    description: record.description,
    status: record.status,
    version: record.version,
    parameters: record.config_json,
    created_at: record.created_at,
    updated_at: record.updated_at,
  };
}

function toServerCreate(input: CreateStrategyInput): Record<string, unknown> {
  return {
    name: input.name.trim(),
    description: input.description?.trim() ?? "",
    module: input.module_family,
    mode: "paper",
    config: input.parameters,
  };
}

function toServerUpdate(input: UpdateStrategyInput): Record<string, unknown> {
  return {
    ...(input.name === undefined ? {} : { name: input.name.trim() }),
    ...(input.description === undefined ? {} : { description: input.description.trim() }),
    ...(input.status === undefined ? {} : { status: input.status }),
    ...(input.parameters === undefined ? {} : { config: input.parameters }),
  };
}

/** Fetch all strategies owned by the tenant. */
export async function listStrategies(module?: string): Promise<StrategyRecord[]> {
  const query = module ? `?module=${encodeURIComponent(module)}` : "";
  const response = await tenantRequest<ServerStrategiesResponse>(`/api/tenant/strategies${query}`);
  return response.items.map(fromServer);
}

/** Fetch a single strategy by its tenant-scoped id. */
export async function getStrategy(id: string): Promise<StrategyRecord> {
  const response = await tenantRequest<ServerStrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`);
  return fromServer(response);
}

/** Create a tenant trading strategy using the Rust API wire contract. */
export async function createStrategy(input: CreateStrategyInput): Promise<StrategyRecord> {
  const response = await tenantRequest<ServerStrategyRecord>("/api/tenant/strategies", {
    method: "POST",
    body: toServerCreate(input),
  });
  return fromServer(response);
}

/** Update a strategy and return the server's canonical record. */
export async function updateStrategy(id: string, input: UpdateStrategyInput): Promise<StrategyRecord> {
  const response = await tenantRequest<ServerStrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: toServerUpdate(input),
  });
  return fromServer(response);
}

/** Archive a strategy. */
export async function archiveStrategy(id: string): Promise<{ archived: boolean; id?: string; status?: string }> {
  return tenantRequest<{ archived: boolean; id?: string; status?: string }>(
    `/api/tenant/strategies/${encodeURIComponent(id)}`,
    { method: "DELETE" },
  );
}
