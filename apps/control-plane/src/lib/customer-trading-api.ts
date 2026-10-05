/**
 * The customer-only trading API client (PROMPT 5 §K, file 94 & Commercial Readiness).
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
 *   renders an honest empty state. Nothing in this client invents values.
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
// Commercial Extended Contracts: Strategies, Backtests, Markets, Configs, etc.
// ---------------------------------------------------------------------------

export interface StrategyRecord {
  id: string;
  organization_id: string;
  name: string;
  description: string;
  module: "sniper" | "copy" | "polymarket";
  mode: "paper" | "live";
  status: "active" | "paused" | "archived";
  version: number;
  config: Record<string, unknown>;
  created_at: string;
  updated_at: string;
  total_trades?: number;
  win_rate_pct?: number;
  net_pnl_usd?: number;
}

export interface StrategiesResponse {
  organization_id: string;
  items: StrategyRecord[];
  count: number;
}

export interface BacktestRecord {
  id: string;
  organization_id: string;
  strategy_id: string;
  strategy_name: string;
  module: string;
  status: "queued" | "running" | "completed" | "failed";
  period_start: string;
  period_end: string;
  venue: string;
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
  created_at: string;
  completed_at: string | null;
  error?: string | null;
  equity_curve?: Array<{ timestamp: string; balance_usd: number }>;
}

export interface BacktestsResponse {
  organization_id: string;
  items: BacktestRecord[];
  count: number;
}

export interface MarketItem {
  id: string;
  symbol: string;
  name: string;
  venue: "raydium" | "pumpfun" | "pumpswap" | "polymarket";
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
  items: MarketItem[];
  count: number;
}

export interface SniperConfig {
  organization_id: string;
  min_liquidity_sol: number;
  max_slippage_bps: number;
  anti_mev_protection: boolean;
  priority_fee_lamports: number;
  entry_amount_sol: number;
  take_profit_pct: number;
  stop_loss_pct: number;
  trailing_stop_pct: number;
  auto_sell_timeout_seconds: number;
  dry_run: boolean;
  blacklisted_tokens: string[];
  dex_routing: "auto" | "raydium_v4" | "pumpfun" | "pumpswap";
  updated_at: string;
}

export interface CopyConfig {
  organization_id: string;
  max_exposure_usd: number;
  allocation_per_trade_sol: number;
  max_slippage_bps: number;
  mirror_buys: boolean;
  mirror_sells: boolean;
  stale_event_timeout_seconds: number;
  allowed_tokens: string[];
  blocked_tokens: string[];
  dry_run: boolean;
  copy_ratio_pct: number;
  updated_at: string;
}

export interface PolymarketConfig {
  organization_id: string;
  active_condition_ids: string[];
  max_position_size_usdc: number;
  max_market_exposure_usdc: number;
  spread_threshold_bps: number;
  reprice_interval_seconds: number;
  cancel_stale_orders: boolean;
  dry_run: boolean;
  order_type: "limit" | "fok" | "gtc";
  updated_at: string;
}

export interface IntegrationItem {
  id: string;
  organization_id: string;
  provider_name: string;
  category: "solana_rpc" | "geyser_feed" | "custody_signer" | "billing" | "alerts" | "other";
  status: "connected" | "degraded" | "not_configured" | "error";
  endpoint_url_masked: string;
  evidence_level: "live_verified" | "simulated" | "not_run";
  last_health_check_at: string | null;
  latency_ms: number | null;
}

export interface IntegrationsResponse {
  organization_id: string;
  items: IntegrationItem[];
  count: number;
}

export interface AnalyticsSummary {
  organization_id: string;
  timeframe: string;
  total_trades: number;
  win_rate_pct: number | null;
  realized_pnl_usd: number;
  unrealized_pnl_usd: number;
  total_volume_usd: number;
  total_fees_usd: number;
  sharpe_ratio: number | null;
  max_drawdown_pct: number | null;
  pnl_series: Array<{ date: string; pnl_usd: number; cumulative_usd: number }>;
  module_breakdown: Array<{
    module: string;
    trades_count: number;
    volume_usd: number;
    pnl_usd: number | null;
    win_rate_pct: number | null;
    fees_usd?: number;
  }>;
  execution_quality: {
    avg_fill_time_ms: number | null;
    avg_slippage_bps: number | null;
    failed_attempts_pct: number | null;
    reverted_txs: number;
  };
}

export interface OnboardingState {
  organization_id: string;
  current_step: number;
  steps_total: number;
  completed: boolean;
  steps: {
    org_created: boolean;
    custody_configured: boolean;
    module_enabled: boolean;
    strategy_configured: boolean;
    paper_trade_executed: boolean;
    live_prerequisites_met: boolean;
  };
  details: {
    has_active_signer: boolean;
    has_funded_wallet: boolean;
    has_enabled_module: boolean;
    has_saved_strategy: boolean;
  };
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

  // --- copy trading specific -----------------------------------------------
  copyLeaders: () => tenantRequest<{ organization_id: string; items: Array<{ address: string; followed_since?: string; note?: string }> }>("/api/tenant/copy/leaders"),
  copyLeaderDetail: (address: string) => tenantRequest<{ leader: Record<string, unknown>; events: unknown[] }>(`/api/tenant/copy/leaders/${encodeURIComponent(address)}`),
  copyLinks: () => tenantRequest<{ organization_id: string; items: unknown[] }>("/api/tenant/copy/links"),

  // --- polymarket specific -------------------------------------------------
  polymarketOrders: (cursor?: string | null) => tenantRequest<{ organization_id: string; items: unknown[]; next_cursor: string | null }>(`/api/tenant/polymarket/orders?limit=${PAGE_SIZE}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`),
  polymarketFills: (since?: string, until?: string) => tenantRequest<{ items: unknown[] }>(`/api/tenant/polymarket/fills?since=${encodeURIComponent(since || new Date(Date.now() - 86400000).toISOString())}&until=${encodeURIComponent(until || new Date().toISOString())}`),
  polymarketReconciliation: () => tenantRequest<{ organization_id: string; drift: unknown[] }>("/api/tenant/polymarket/reconciliation"),

  // --- strategies ----------------------------------------------------------
  strategies: () => tenantRequest<StrategiesResponse>("/api/tenant/strategies"),
  strategyDetail: (id: string) =>
    tenantRequest<StrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`),
  createStrategy: (strategy: Partial<StrategyRecord>) =>
    tenantRequest<StrategyRecord>("/api/tenant/strategies", {
      method: "POST",
      body: strategy,
    }),
  updateStrategy: (id: string, strategy: Partial<StrategyRecord>) =>
    tenantRequest<StrategyRecord>(`/api/tenant/strategies/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: strategy,
    }),
  archiveStrategy: (id: string) =>
    tenantRequest<{ archived: boolean }>(`/api/tenant/strategies/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),

  // --- backtests -----------------------------------------------------------
  backtests: () => tenantRequest<BacktestsResponse>("/api/tenant/backtests"),
  backtestDetail: (id: string) =>
    tenantRequest<BacktestRecord>(`/api/tenant/backtests/${encodeURIComponent(id)}`),
  createBacktest: (params: {
    strategy_id: string;
    period_start: string;
    period_end: string;
    venue: string;
    initial_balance_usd: number;
    fee_rate_bps: number;
    slippage_bps: number;
  }) =>
    tenantRequest<BacktestRecord>("/api/tenant/backtests", {
      method: "POST",
      body: params,
    }),

  // --- markets -------------------------------------------------------------
  markets: () => tenantRequest<MarketsResponse>("/api/tenant/markets"),

  // --- module configurations -----------------------------------------------
  sniperConfig: () => tenantRequest<SniperConfig | null>("/api/tenant/sniper/config"),
  updateSniperConfig: (config: Partial<SniperConfig>) =>
    tenantRequest<SniperConfig>("/api/tenant/sniper/config", {
      method: "PUT",
      body: config,
    }),

  copyConfig: () => tenantRequest<CopyConfig | null>("/api/tenant/copy/config"),
  updateCopyConfig: (config: Partial<CopyConfig>) =>
    tenantRequest<CopyConfig>("/api/tenant/copy/config", {
      method: "PUT",
      body: config,
    }),

  polymarketConfig: () => tenantRequest<PolymarketConfig | null>("/api/tenant/polymarket/config"),
  updatePolymarketConfig: (config: Partial<PolymarketConfig>) =>
    tenantRequest<PolymarketConfig>("/api/tenant/polymarket/config", {
      method: "PUT",
      body: config,
    }),

  // --- integrations catalog ------------------------------------------------
  integrations: () => tenantRequest<IntegrationsResponse>("/api/tenant/integrations"),

  // --- analytics -----------------------------------------------------------
  analytics: (timeframe: "24h" | "7d" | "30d" | "all" = "7d") =>
    tenantRequest<AnalyticsSummary>(`/api/tenant/analytics?timeframe=${encodeURIComponent(timeframe)}`),

  // --- onboarding ----------------------------------------------------------
  onboardingState: () => tenantRequest<OnboardingState>("/api/tenant/onboarding"),
  completeOnboardingStep: (step: number) =>
    tenantRequest<OnboardingState>("/api/tenant/onboarding/complete", {
      method: "POST",
      body: { step },
    }),
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
      return "No data recorded yet — this view populates as your organization executes strategies.";
    case "ready":
      return "";
    case "suspended_tenant":
      return `Your organization's trading access is currently suspended (${state.reason}). Contact support or review your billing status to reactivate.`;
    case "entitlement_denied":
      return `This module is not included in your current subscription tier (${state.reason}). Upgrade to request access to additional entitled features.`;
    case "module_disabled":
      return `This trading module is currently paused${state.reason ? `: ${state.reason}` : ""}. You can re-enable it from the module controls.`;
    case "stale_runtime":
      return `No active runtime engine is currently scheduled for this module (${state.reason}). Configurations remain saved.`;
    case "plane_unavailable":
      return `The trading data plane is not attached to this deployment (${state.reason}). Production data is strictly guarded and never simulated.`;
    case "custody_unavailable":
      return `Custody / Key Management service is unreachable (${state.reason}). Signing-dependent executions fail closed until resolved.`;
    case "error":
      return `Operation failed: ${state.reason}`;
  }
}

/** Is this state one the user can retry (transient)? */
export function isRetryable(state: TradingSurfaceState): boolean {
  return state.kind === "error" || state.kind === "plane_unavailable" || state.kind === "custody_unavailable";
}

export function toDisplayError(e: unknown): string {
  if (e instanceof ApiError) return `${e.kind}: ${e.reason}`;
  if (e instanceof Error) return e.message;
  return 'request failed';
}
