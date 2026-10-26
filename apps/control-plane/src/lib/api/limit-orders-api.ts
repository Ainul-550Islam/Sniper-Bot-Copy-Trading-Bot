/**
 * Tenant limit-order API client.
 *
 * Uses `tenantRequest`, so customer pages cannot address a non-tenant path.
 * Types mirror the server's create body, `OrderView`, list envelope and
 * cancellation response. Status shown by callers is always the status from
 * a server response or a refetch, never an optimistic local status.
 */

import { tenantRequest } from "../customer-trading-api";

export type LimitOrderSide = "buy" | "sell";
export type LimitOrderTrigger = "at_or_below" | "at_or_above";
export type LimitOrderStatus = "active" | "triggered" | "cancelled" | "expired";

export interface LimitOrder {
  id: string;
  mint: string;
  side: LimitOrderSide;
  trigger: LimitOrderTrigger;
  price_sol: number;
  amount: number;
  status: LimitOrderStatus;
  created_at: string;
  expires_at: string | null;
  resolved_at: string | null;
  attempts: number;
  last_error: string | null;
}

export interface LimitOrdersResponse {
  organization_id: string;
  orders: LimitOrder[];
  next_before: string | null;
}

export interface CreateLimitOrderInput {
  mint: string;
  side: LimitOrderSide;
  trigger: LimitOrderTrigger;
  price_sol: number;
  amount: number;
  expires_in_secs?: number;
}

export interface CreateLimitOrderResponse {
  id: string;
  organization_id: string;
  status: "active";
}

export interface CancelLimitOrderResponse {
  id: string;
  status: "cancelled";
}

export interface ListLimitOrdersOptions {
  status?: LimitOrderStatus;
  limit?: number;
  before?: string;
}

export const limitOrdersApi = {
  list: (options: ListLimitOrdersOptions = {}) => {
    const query = new URLSearchParams();
    if (options.status) query.set("status", options.status);
    if (options.limit !== undefined) query.set("limit", String(options.limit));
    if (options.before) query.set("before", options.before);
    const queryString = query.toString();
    const suffix = queryString ? `?${queryString}` : "";
    return tenantRequest<LimitOrdersResponse>(
      `/api/tenant/limit-orders${suffix}`,
    );
  },
  create: (input: CreateLimitOrderInput) =>
    tenantRequest<CreateLimitOrderResponse>("/api/tenant/limit-orders", {
      method: "POST",
      body: input,
    }),
  cancel: (id: string) =>
    tenantRequest<CancelLimitOrderResponse>(
      `/api/tenant/limit-orders/${encodeURIComponent(id)}/cancel`,
      { method: "POST" },
    ),
};
