/**
 * Tenant DCA schedule API client.
 *
 * Uses `tenantRequest`, so every path is mechanically restricted to
 * `/api/tenant/...`. Types mirror the server's `ScheduleView`, create body,
 * transition responses and run-ledger rows. The client never synthesizes a
 * schedule or a run: callers render exactly what the server returned.
 */

import { tenantRequest } from "../customer-trading-api";

export type DcaScheduleStatus = "active" | "completed" | "paused" | "cancelled";

export interface DcaSchedule {
  id: string;
  mint: string;
  amount_per_run_sol: number;
  interval_secs: number;
  budget_sol: number;
  spent_sol: number;
  remaining_sol: number;
  status: DcaScheduleStatus;
  next_run_at: string;
  last_run_at: string | null;
  attempts: number;
  last_error: string | null;
  created_at: string;
}

export interface DcaSchedulesResponse {
  organization_id: string;
  schedules: DcaSchedule[];
}

export interface CreateDcaScheduleInput {
  mint: string;
  amount_per_run_sol: number;
  interval_secs: number;
  budget_sol: number;
  start_in_secs?: number;
}

export interface CreateDcaScheduleResponse {
  id: string;
  status: "active";
  next_run_at: string;
}

export interface DcaTransitionResponse {
  id: string;
  status: DcaScheduleStatus;
}

export interface DcaRun {
  run_id: number;
  slot_at: string;
  amount_sol: number;
  status: string;
  created_at: string;
}

export interface DcaRunsResponse {
  schedule_id: string;
  runs: DcaRun[];
}

export interface ListDcaSchedulesOptions {
  status?: DcaScheduleStatus;
  limit?: number;
}

export const dcaApi = {
  list: (options: ListDcaSchedulesOptions = {}) => {
    const query = new URLSearchParams();
    if (options.status) query.set("status", options.status);
    if (options.limit !== undefined) query.set("limit", String(options.limit));
    const queryString = query.toString();
    const suffix = queryString ? `?${queryString}` : "";
    return tenantRequest<DcaSchedulesResponse>(
      `/api/tenant/dca-schedules${suffix}`,
    );
  },
  create: (input: CreateDcaScheduleInput) =>
    tenantRequest<CreateDcaScheduleResponse>("/api/tenant/dca-schedules", {
      method: "POST",
      body: input,
    }),
  pause: (id: string) =>
    tenantRequest<DcaTransitionResponse>(
      `/api/tenant/dca-schedules/${encodeURIComponent(id)}/pause`,
      { method: "POST" },
    ),
  resume: (id: string) =>
    tenantRequest<DcaTransitionResponse>(
      `/api/tenant/dca-schedules/${encodeURIComponent(id)}/resume`,
      { method: "POST" },
    ),
  cancel: (id: string) =>
    tenantRequest<DcaTransitionResponse>(
      `/api/tenant/dca-schedules/${encodeURIComponent(id)}/cancel`,
      { method: "POST" },
    ),
  runs: (id: string) =>
    tenantRequest<DcaRunsResponse>(
      `/api/tenant/dca-schedules/${encodeURIComponent(id)}/runs`,
    ),
};
