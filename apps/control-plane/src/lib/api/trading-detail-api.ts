/**
 * Tenant trading detail API client (Part 04, L4).
 *
 * Uses `tenantRequest`, so every path is mechanically restricted to
 * `/api/tenant/…`. Covers the detail endpoints that had OpenAPI contracts
 * but no frontend caller:
 *
 *   GET /api/tenant/trades, GET /api/tenant/trades/{position_id},
 *   GET /api/tenant/positions/{id}, GET /api/tenant/orders/{id},
 *   GET /api/tenant/executions/{id}, GET /api/tenant/balances,
 *   GET /api/tenant/recovery/intents, GET /api/tenant/recovery/sweep,
 *   GET /api/tenant/transactions/{signature},
 *   GET /api/tenant/polymarket/orders/{venue_order_id}
 *
 * Numbers and PnL render exactly what the server returned: a `null` stays
 * "—", never a client-side recomputation or estimate.
 */

import { tenantRequest } from "../customer-trading-api";
import type {
  TenantExecution,
  TenantOrder,
  TenantPosition,
} from "../customer-trading-api";

export interface TenantTrade {
  id: string;
  position_id: string;
  symbol: string;
  side: string;
  qty: number;
  price: number | null;
  venue: string;
  status: string;
  executed_at: string;
}

export interface TradesResponse {
  organization_id: string;
  items: TenantTrade[];
  count: number;
}

export interface TenantBalance {
  asset: string;
  symbol: string;
  available: number | null;
  locked: number | null;
  value_usd_cents: number | null;
  updated_at: string;
}

export interface BalancesResponse {
  organization_id: string;
  items: TenantBalance[];
}

export interface RecoveryIntent {
  id: string;
  intent: string;
  state: string;
  reason: string | null;
  created_at: string;
}

export interface RecoveryIntentsResponse {
  organization_id: string;
  items: RecoveryIntent[];
}

export interface RecoverySweep {
  id: string;
  signature: string | null;
  state: string;
  amount_usd_cents: number | null;
  created_at: string;
}

export interface RecoverySweepResponse {
  organization_id: string;
  items: RecoverySweep[];
}

export interface TransactionDetail {
  signature: string;
  status: string;
  slot: number | null;
  fee_lamports: number | null;
  confirmed_at: string | null;
  memo: string | null;
}

export interface PolymarketOrderDetail {
  venue_order_id: string;
  market: string | null;
  side: string | null;
  status: string | null;
  price: number | null;
  size: number | null;
  created_at: string | null;
}

export const tradingDetailApi = {
  trades: () => tenantRequest<TradesResponse>("/api/tenant/trades"),

  tradeForPosition: (positionId: string) =>
    tenantRequest<{ position_id: string; trades: TenantTrade[] }>(
      `/api/tenant/trades/${encodeURIComponent(positionId)}`,
    ),

  position: (id: string) =>
    tenantRequest<TenantPosition>(`/api/tenant/positions/${encodeURIComponent(id)}`),

  order: (id: string) =>
    tenantRequest<TenantOrder>(`/api/tenant/orders/${encodeURIComponent(id)}`),

  execution: (id: string) =>
    tenantRequest<TenantExecution>(`/api/tenant/executions/${encodeURIComponent(id)}`),

  balances: () => tenantRequest<BalancesResponse>("/api/tenant/balances"),

  recoveryIntents: () =>
    tenantRequest<RecoveryIntentsResponse>("/api/tenant/recovery/intents"),

  recoverySweep: () =>
    tenantRequest<RecoverySweepResponse>("/api/tenant/recovery/sweep"),

  transaction: (signature: string) =>
    tenantRequest<TransactionDetail>(
      `/api/tenant/transactions/${encodeURIComponent(signature)}`,
    ),

  polymarketOrder: (venueOrderId: string) =>
    tenantRequest<PolymarketOrderDetail>(
      `/api/tenant/polymarket/orders/${encodeURIComponent(venueOrderId)}`,
    ),
};
