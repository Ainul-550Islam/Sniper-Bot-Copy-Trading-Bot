/**
 * Operator Control Plane API Client.
 *
 * Exposes strictly typed methods for every operator-only route registered
 * in the server crate (`crates/server/src/api.rs`).
 *
 * Rules:
 * - Uses shared `request<T>()` from `lib/api.ts`.
 * - Propagates backend `ApiError` instances.
 * - ZERO localStorage / sessionStorage usage (credentials remain in memory only).
 * - Plaintext API keys are never echoed or retained.
 *
 * CREDENTIAL MODEL (Round 2, defect D1): every route below is an OPERATOR
 * route on the backend. `require_role` authenticates it with `extract_key`,
 * which reads `x-api-key` (then `Authorization: Bearer …`) and resolves it
 * through the operator key registry by SHA-256. The SaaS session token is a
 * different credential for a different plane and is rejected with 401 here.
 *
 * So every call:
 *   - sends `x-api-key: <operator key>` from the in-memory operator store;
 *   - passes `anonymous: true` so `request()` attaches NEITHER the session
 *     `Authorization` header NOR the `x-organization` tenant hint;
 *   - throws {@link OperatorKeyRequiredError} locally when the console is
 *     locked, so no request leaves the browser without a key;
 *   - clears the stored key on a 401, which returns the UI to the unlock
 *     prompt instead of retrying a credential the backend has refused.
 */

import { request, ApiError, type RequestOptions } from "../api";
import { clearOperatorKey, getOperatorKey } from "../operator-session";

export { ApiError };

export interface StatusResponse {
  execution_mode: string;
  live_allowed: boolean;
  kill_switch: boolean;
}

export interface HealthResponse {
  ok: boolean;
}

export interface ModeResponse {
  ok: boolean;
  mode: string;
  live_allowed: boolean;
  note?: string;
}

export interface ModuleStatus {
  module: string;
  enabled: boolean;
  running: boolean;
  healthy: boolean;
  connected: boolean;
  events_seen: number;
  signals_generated: number;
  orders_sent: number;
  orders_filled: number;
  orders_failed: number;
  orders_rejected_by_risk: number;
  realized_pnl: number;
  unrealized_pnl: number;
  consecutive_errors: number;
  last_error?: string | null;
  last_heartbeat?: string | null;
  last_event?: string | null;
  detail?: string | null;
}

export interface ModuleToggleResponse {
  ok: boolean;
  module: string;
  enabled: boolean;
}

export interface KillResponse {
  ok: boolean;
  kill_switch: boolean;
}

export interface ResumeResponse {
  ok: boolean;
  kill_switch: boolean;
}

export interface RiskKillSwitchInput {
  scope: string;
  engaged: boolean;
  reason?: string;
}

export interface RiskKillSwitchResponse {
  ok: boolean;
  scope: string;
  outcome: string;
  active: boolean;
}

export interface OperatorKey {
  label: string;
  role: string;
  key_hash?: string;
  hash?: string;
  source?: string;
  enabled?: boolean;
  created_at?: string;
  last_used_at?: string | null;
}

export interface KeysListResponse {
  keys: OperatorKey[];
}

export interface AddKeyInput {
  label: string;
  key: string;
  role: string;
}

export interface AddKeyResponse {
  ok: boolean;
  label: string;
  role: string;
  key_hash?: string;
  hash?: string;
}

export interface RevokeKeyResponse {
  ok: boolean;
  revoked: string;
}

export interface AuditRecord {
  id: number;
  ts: string;
  actor: string;
  action: string;
  target: string | null;
  outcome: string;
  detail: Record<string, unknown>;
  hash: string;
  chained: boolean;
}

export interface AuditListResponse {
  durable: boolean;
  count: number;
  records: AuditRecord[];
}

export interface AuditVerifyResponse {
  status: "intact" | "broken" | "not_chained" | "unknown" | string;
  entries?: number;
  at_id?: number;
  error?: string;
}

export interface JournalStatus {
  available: boolean;
  dir?: string;
  trades_bytes?: number;
  positions_bytes?: number;
  events_bytes?: number;
}

export interface JournalRotateResponse {
  ok: boolean;
  rotated: string[];
  errors: string[];
}

export interface DbStatus {
  database: "connected" | "disabled" | string;
  pool?: {
    size: number;
    idle: number;
    active: number;
    max: number;
  };
  migrations_applied?: number;
  dedup_backend?: string;
  audit_durable?: boolean;
}

export interface ReconciliationFinding {
  finding_id: string;
  kind: string;
  module?: string | null;
  venue?: string | null;
  asset?: string | null;
  position_id?: string | null;
  event_id?: string | null;
  trade_id?: string | null;
  order_id?: string | null;
  expected: number;
  actual: number;
  detail: string;
}

export interface ReconciliationFindingsResponse {
  count: number;
  findings: ReconciliationFinding[];
}

export interface RecoveryFailedResponse {
  available: boolean;
  count?: number;
  items: unknown[];
}

export interface ExecutionsResponse {
  count: number;
  states: Record<string, number>;
  executions: unknown[];
}

export interface WalletsResponse {
  available: boolean;
  wallets: unknown[];
}

export interface AccountingEventsResponse {
  count: number;
  total: number;
  by_kind: Record<string, number>;
  pending: number;
  events: unknown[];
}

export interface AccountingEventInput {
  kind: string;
  wallet: string;
  quote_asset: string;
  reference_id: string;
  asset?: string;
  side?: string;
  quantity?: number;
  price?: number;
  quote_amount?: number;
  fee?: number;
  correlation_id?: string;
  position_id?: string;
  counterparty_wallet?: string;
  venue?: string;
  strategy?: string;
  detail?: string;
}

export interface PostAccountingEventResponse {
  ok: boolean;
  outcome: string;
  event_id?: string;
  effect?: unknown;
  reason?: string;
}

// ---------------------------------------------------------------------------
// Operator credential plumbing
// ---------------------------------------------------------------------------

/**
 * Thrown BEFORE a request is made when no operator key is in memory.
 *
 * It is deliberately not an {@link ApiError}: no HTTP response exists yet, and
 * the UI uses the type to render the unlock prompt instead of an error banner.
 */
export class OperatorKeyRequiredError extends Error {
  constructor() {
    super(
      "operator_key_required: unlock the operator console with an operator API key",
    );
    this.name = "OperatorKeyRequiredError";
  }
}

/**
 * One typed call to an operator route.
 *
 * `anonymous: true` is not optional: without it `request()` would attach the
 * tenant session token and the `x-organization` hint, which the operator plane
 * does not accept (and which would leak the tenant session to a route that
 * logs presented keys by hash).
 */
async function opRequest<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const key = getOperatorKey();
  if (key === null) {
    throw new OperatorKeyRequiredError();
  }
  try {
    return await request<T>(path, {
      ...options,
      anonymous: true,
      headers: { ...(options.headers ?? {}), "x-api-key": key },
    });
  } catch (error) {
    // A 401 means the backend refused this exact key (unknown, revoked, or
    // insufficient role). Drop it — keeping a refused credential would only
    // produce the same 401 on every retry — and keep the server's reason so the
    // unlock prompt can show it.
    if (error instanceof ApiError && error.status === 401) {
      clearOperatorKey(error.reason);
    }
    throw error;
  }
}

// ---------------------------------------------------------------------------
// Operator API Functions
// ---------------------------------------------------------------------------

export async function getStatus(): Promise<StatusResponse> {
  return opRequest<StatusResponse>("/api/status");
}

export async function getHealth(): Promise<HealthResponse> {
  return opRequest<HealthResponse>("/api/health");
}

export async function setMode(mode: string): Promise<ModeResponse> {
  return opRequest<ModeResponse>("/api/mode", {
    method: "POST",
    body: { mode },
  });
}

export async function getModules(): Promise<ModuleStatus[]> {
  return opRequest<ModuleStatus[]>("/api/modules");
}

export async function enableModule(name: string): Promise<ModuleToggleResponse> {
  return opRequest<ModuleToggleResponse>(`/api/modules/${encodeURIComponent(name)}/enable`, {
    method: "POST",
  });
}

export async function disableModule(name: string): Promise<ModuleToggleResponse> {
  return opRequest<ModuleToggleResponse>(`/api/modules/${encodeURIComponent(name)}/disable`, {
    method: "POST",
  });
}

export async function killSystem(): Promise<KillResponse> {
  return opRequest<KillResponse>("/api/kill", {
    method: "POST",
  });
}

export async function resumeSystem(): Promise<ResumeResponse> {
  return opRequest<ResumeResponse>("/api/resume", {
    method: "POST",
  });
}

export async function setRiskKillSwitch(input: RiskKillSwitchInput): Promise<RiskKillSwitchResponse> {
  return opRequest<RiskKillSwitchResponse>("/api/risk/kill-switch", {
    method: "POST",
    body: input,
  });
}

export async function getKeys(): Promise<KeysListResponse> {
  return opRequest<KeysListResponse>("/api/keys");
}

export async function addKey(input: AddKeyInput): Promise<AddKeyResponse> {
  return opRequest<AddKeyResponse>("/api/keys", {
    method: "POST",
    body: input,
  });
}

export async function revokeKey(hash: string): Promise<RevokeKeyResponse> {
  return opRequest<RevokeKeyResponse>(`/api/keys/${encodeURIComponent(hash)}`, {
    method: "DELETE",
  });
}

export async function getAuditTrail(limit?: number): Promise<AuditListResponse> {
  const query = limit ? `?limit=${encodeURIComponent(limit)}` : "";
  return opRequest<AuditListResponse>(`/api/audit${query}`);
}

export async function verifyAuditChain(): Promise<AuditVerifyResponse> {
  return opRequest<AuditVerifyResponse>("/api/audit/verify");
}

export async function getJournalStatus(): Promise<JournalStatus> {
  return opRequest<JournalStatus>("/api/journal");
}

export async function rotateJournal(): Promise<JournalRotateResponse> {
  return opRequest<JournalRotateResponse>("/api/journal", {
    method: "POST",
  });
}

export async function getDbStatus(): Promise<DbStatus> {
  return opRequest<DbStatus>("/api/db");
}

export async function getConfig(): Promise<Record<string, unknown>> {
  return opRequest<Record<string, unknown>>("/api/config");
}

export async function getReconciliationFindings(limit?: number): Promise<ReconciliationFindingsResponse> {
  const query = limit ? `?limit=${encodeURIComponent(limit)}` : "";
  return opRequest<ReconciliationFindingsResponse>(`/api/reconciliation/findings${query}`);
}

export async function getRecoveryFailed(): Promise<RecoveryFailedResponse> {
  return opRequest<RecoveryFailedResponse>("/api/recovery/failed");
}

export async function getExecutions(params?: {
  limit?: number;
  state?: string;
  open?: boolean;
}): Promise<ExecutionsResponse> {
  const searchParams = new URLSearchParams();
  if (params?.limit !== undefined) searchParams.set("limit", String(params.limit));
  if (params?.state !== undefined) searchParams.set("state", params.state);
  if (params?.open !== undefined) searchParams.set("open", String(params.open));
  const query = searchParams.toString() ? `?${searchParams.toString()}` : "";
  return opRequest<ExecutionsResponse>(`/api/executions${query}`);
}

export async function getExecutionById(id: string): Promise<Record<string, unknown>> {
  return opRequest<Record<string, unknown>>(`/api/executions/${encodeURIComponent(id)}`);
}

export async function getTrades(limit?: number): Promise<unknown[]> {
  const query = limit ? `?limit=${encodeURIComponent(limit)}` : "";
  return opRequest<unknown[]>(`/api/trades${query}`);
}

export async function getWallets(): Promise<WalletsResponse> {
  return opRequest<WalletsResponse>("/api/wallets");
}

export async function getAccountingEvents(limit?: number): Promise<AccountingEventsResponse> {
  const query = limit ? `?limit=${encodeURIComponent(limit)}` : "";
  return opRequest<AccountingEventsResponse>(`/api/accounting/events${query}`);
}

export async function postAccountingEvent(input: AccountingEventInput): Promise<PostAccountingEventResponse> {
  return opRequest<PostAccountingEventResponse>("/api/accounting/events", {
    method: "POST",
    body: input,
  });
}

export async function getOrderById(id: string): Promise<Record<string, unknown>> {
  return opRequest<Record<string, unknown>>(`/api/orders/${encodeURIComponent(id)}`);
}

export function getEventsWsPath(): string {
  return "/api/events";
}

export const operatorApi = {
  getStatus,
  getHealth,
  setMode,
  getModules,
  enableModule,
  disableModule,
  killSystem,
  resumeSystem,
  setRiskKillSwitch,
  getKeys,
  addKey,
  revokeKey,
  getAuditTrail,
  verifyAuditChain,
  getJournalStatus,
  rotateJournal,
  getDbStatus,
  getConfig,
  getReconciliationFindings,
  getRecoveryFailed,
  getExecutions,
  getExecutionById,
  getTrades,
  getWallets,
  getAccountingEvents,
  postAccountingEvent,
  getOrderById,
  getEventsWsPath,
};

export default operatorApi;
