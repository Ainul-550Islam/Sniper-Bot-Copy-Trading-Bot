/**
 * The customer-only trading API client (PROMPT 5 §K, file 94).
 *
 * HARD RULES THIS MODULE ENFORCES:
 *
 * - It can ONLY call tenant customer endpoints (`/api/tenant/...`).
 *   `tenantRequest` rejects any other path with a developer-facing
 *   error before a single byte leaves the browser: a customer page can
 *   never reach an operator-global endpoint (`/api/saas/...`,
 *   `/api/ops/...`, `/api/admin/...`), not by bug and not by
 *   construction.
 * - Every response is typed; every refusal is an {@link ApiError}
 *   carrying the backend's stable `error` kind.
 * - Every trading surface state the customer UI must render is
 *   classifiable from here: loading, empty, generic error, suspended
 *   tenant, entitlement denial, module disabled, stale runtime
 *   (no registered runtime), trading-plane unavailable, custody
 *   unavailable.
 * - No synthetic numbers ever: if the API returns no data, the caller
 *   renders an empty state. Nothing in this client invents values.
 */

import { ApiError, request } from "./api";

/** The one path prefix a customer trading page may call. */
const TENANT_PREFIX = "/api/tenant/";

/**
 * A typed call to a TENANT customer endpoint. Any path outside
 * `/api/tenant/…` throws before fetching (see module docs).
 */
export async function tenantRequest<T>(
  path: string,
  options: Parameters<typeof request>[1] = {},
): Promise<T> {
  if (!path.startsWith(TENANT_PREFIX)) {
    throw new Error(
      `customer-trading-api: refusing non-tenant path '${path}' — customer pages may only call ${TENANT_PREFIX}… endpoints`,
    );
  }
  return request<T>(path, options);
}

// ---------------------------------------------------------------------------
// Response types (mirrors of the server's JSON contracts)
// ---------------------------------------------------------------------------

/** One row of `GET /api/tenant/bots`. */
export interface BotRuntimeRow {
  module: string;
  runtime_id: string;
  generation: string;
  phase: string;
  since: string;
  controls: string;
}

/** `GET /api/tenant/bots`. */
export interface BotsResponse {
  organization_id: string;
  items: BotRuntimeRow[];
  count: number;
}

/** One order (`GET /api/tenant/orders`). */
export interface TenantOrder {
  id: string;
  idempotency_key: string;
  module: string;
  side: string;
  symbol: string;
  venue: string;
  mode: string;
  status: string;
  qty: number;
  price: number | null;
  external_id: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
  submitted_at: string | null;
  finished_at: string | null;
}

/** `GET /api/tenant/orders?limit=&cursor=` (keyset pagination). */
export interface OrdersPage {
  organization_id: string;
  items: TenantOrder[];
  next_cursor: string | null;
}

/** One position (`GET /api/tenant/positions`). */
export interface TenantPosition {
  id: string;
  symbol: string;
  side: string;
  qty: number;
  entry_price: number | null;
  mark_price: number | null;
  realized_pnl: number | null;
  unrealized_pnl: number | null;
  status: string;
  opened_at: string | null;
  closed_at: string | null;
}

/** `GET /api/tenant/positions?limit=&cursor=` (keyset pagination). */
export interface PositionsPage {
  organization_id: string;
  items: TenantPosition[];
  next_cursor: string | null;
}

/** One execution row (`GET /api/tenant/executions`). */
export interface TenantExecution {
  id: string;
  order_id: string;
  kind: string;
  status: string;
  qty: number | null;
  price: number | null;
  signature: string | null;
  error: string | null;
  at: string;
}

/** `GET /api/tenant/executions?since=&until=` or `?order_id=`. */
export interface ExecutionsPage {
  organization_id: string;
  items: TenantExecution[];
}

/** `GET /api/tenant/reports/pnl` (default: today, UTC). */
export interface PnlResponse {
  organization_id: string;
  pnl: number;
}

/** `GET /api/tenant/reports/summary`. */
export interface ReportsSummaryResponse {
  summary: Record<string, unknown>;
}

/** `GET /api/tenant/{module}/status` (sniper / copy / polymarket). */
export interface ModuleStatusResponse {
  organization_id: string;
  module: string;
  entitlement: { feature: string | null; granted: boolean; note?: string };
  tenant_override: {
    state: string;
    reason: string;
    updated_at: string;
    updated_by: string;
  } | null;
  effective_state: "enabled" | "disabled";
  runtime: {
    runtime_id: string;
    generation: string;
    phase: string;
    since: string;
  } | null;
  runtime_detail: string | null;
  controls: { available: boolean; actions: string[]; scope: string };
  /** Present on control responses. */
  control_result?: {
    action: string;
    applied: boolean;
    detail: string;
    reason?: string;
    updated_at: string;
    updated_by: string;
  };
}

/** `GET /api/tenant/telegram/status`. */
export interface TelegramStatusResponse {
  organization_id: string;
  module: string;
  entitlement: { feature: string | null; granted: boolean; note?: string };
  effective_state: string;
  runtime: { runtime_id: string; generation: string; phase: string; since: string } | null;
  runtime_detail: string | null;
  binding: { chat_id: number; bound_at: string; bound_by: string } | null;
  binding_detail: { purpose: string; delivery: string };
  controls: { available: boolean; actions: string[] };
}

// ---------------------------------------------------------------------------
// The typed client surface
// ---------------------------------------------------------------------------

/** Page size for server-paginated tables. */
export const PAGE_SIZE = 25;

export const customerTrading = {
  // --- runtime / bots -----------------------------------------------------
  bots: () => tenantRequest<BotsResponse>("/api/tenant/bots"),
  botDetail: (module: string) =>
    tenantRequest<{ organization_id: string; bot: BotRuntimeRow | null; detail?: string }>(
      `/api/tenant/bots/${encodeURIComponent(module)}`,
    ),

  // --- orders / positions / executions ------------------------------------
  orders: (cursor?: string | null) =>
    tenantRequest<OrdersPage>(
      `/api/tenant/orders?limit=${PAGE_SIZE}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`,
    ),
  cancelOrder: (id: string) =>
    tenantRequest<{ cancelled: string }>(`/api/tenant/orders/${encodeURIComponent(id)}/cancel`, {
      method: "POST",
    }),
  positions: (cursor?: string | null) =>
    tenantRequest<PositionsPage>(
      `/api/tenant/positions?limit=${PAGE_SIZE}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`,
    ),
  executionsForOrder: (orderId: string) =>
    tenantRequest<ExecutionsPage>(
      `/api/tenant/executions?order_id=${encodeURIComponent(orderId)}`,
    ),
  executionsWindow: (since: string, until: string) =>
    tenantRequest<ExecutionsPage>(
      `/api/tenant/executions?since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
    ),

  // --- reports ------------------------------------------------------------
  pnlToday: () => tenantRequest<PnlResponse>("/api/tenant/reports/pnl"),
  reportsSummary: () => tenantRequest<ReportsSummaryResponse>("/api/tenant/reports/summary"),

  // --- module status/controls (§J 74–77) ----------------------------------
  sniperStatus: () => tenantRequest<ModuleStatusResponse>("/api/tenant/sniper/status"),
  copyStatus: () => tenantRequest<ModuleStatusResponse>("/api/tenant/copy/status"),
  polymarketStatus: () => tenantRequest<ModuleStatusResponse>("/api/tenant/polymarket/status"),
  moduleControl: (
    module: "sniper" | "copy" | "polymarket",
    action: "enable" | "disable",
    reason?: string,
  ) =>
    tenantRequest<ModuleStatusResponse>(`/api/tenant/${module}/controls`, {
      method: "POST",
      body: reason === undefined ? { action } : { action, reason },
    }),

  // --- telegram binding/status (§J 77) -------------------------------------
  telegramStatus: () => tenantRequest<TelegramStatusResponse>("/api/tenant/telegram/status"),
  bindTelegram: (chatId: number) =>
    tenantRequest<{ organization_id: string; binding: { chat_id: number; bound_at: string; bound_by: string }; detail: string }>(
      "/api/tenant/telegram/binding",
      { method: "PUT", body: { chat_id: chatId } },
    ),
  unbindTelegram: () =>
    tenantRequest<{ organization_id: string; binding: null; detail: string }>(
      "/api/tenant/telegram/binding",
      { method: "DELETE" },
    ),
};

// ---------------------------------------------------------------------------
// Trading-surface state classification
// ---------------------------------------------------------------------------

/** Every state a customer trading surface can be in. */
export type TradingSurfaceState =
  | { kind: "loading" }
  | { kind: "empty" }
  | { kind: "ready" }
  | { kind: "suspended_tenant"; reason: string }
  | { kind: "entitlement_denied"; reason: string }
  | { kind: "module_disabled"; reason: string }
  | { kind: "stale_runtime"; reason: string }
  | { kind: "plane_unavailable"; reason: string }
  | { kind: "custody_unavailable"; reason: string }
  | { kind: "error"; reason: string };

/** Classify an API refusal into the customer-facing trading state. */
export function classifyTradingError(e: unknown): TradingSurfaceState {
  if (e instanceof ApiError) {
    switch (e.kind) {
      case "tenant_lifecycle_blocked":
        return { kind: "suspended_tenant", reason: e.reason };
      case "module_not_entitled":
        return { kind: "entitlement_denied", reason: e.reason };
      case "trading_data_plane_unavailable":
        return { kind: "plane_unavailable", reason: e.reason };
      default:
        if (e.kind.includes("custody")) {
          return { kind: "custody_unavailable", reason: `${e.kind}: ${e.reason}` };
        }
        return { kind: "error", reason: `${e.kind}: ${e.reason}` };
    }
  }
  if (e instanceof Error) return { kind: "error", reason: e.message };
  return { kind: "error", reason: "request failed" };
}

/** Human text for every non-ready trading surface state. */
export function tradingStateMessage(state: TradingSurfaceState): string {
  switch (state.kind) {
    case "loading":
      return "Loading…";
    case "empty":
      return "No data yet — this space fills in as your organization trades.";
    case "ready":
      return "";
    case "suspended_tenant":
      return `Your organization's data access is currently blocked (${state.reason}). Resolve the account state with the operator to restore trading surfaces.`;
    case "entitlement_denied":
      return `This module is not part of your current plan (${state.reason}). Upgrade or contact the operator to enable it.`;
    case "module_disabled":
      return `This module is disabled for your organization${state.reason ? `: ${state.reason}` : ""}. You can re-enable it from its controls.`;
    case "stale_runtime":
      return `No runtime is currently registered for this module (${state.reason}). Your configuration is intact; the runtime reconnects when the operator's deployment schedules it.`;
    case "plane_unavailable":
      return `The trading data plane is not attached to this deployment (${state.reason}). No data can be shown — this is never silently faked.`;
    case "custody_unavailable":
      return `Custody is currently unavailable (${state.reason}). Signing-dependent surfaces stay blocked until the custody provider is reachable.`;
    case "error":
      return `Request failed: ${state.reason}`;
  }
}

/** Is this state one the user can retry (transient)? */
export function isRetryable(state: TradingSurfaceState): boolean {
  return state.kind === "error" || state.kind === "plane_unavailable" || state.kind === "custody_unavailable";
}
