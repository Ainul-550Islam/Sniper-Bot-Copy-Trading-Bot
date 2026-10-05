/**
 * Typed Risk Limits & Kill-Switch Client (THIRD.md §127).
 *
 * Interfaces with `/api/saas/risk-dashboard` and emergency kill-switches.
 */

import { request } from "../api";

export interface RiskLimitRule {
  id: string;
  name: string;
  scope: "tenant" | "strategy" | "wallet" | "global";
  limit_usd_cents: number;
  current_utilization_cents: number;
  utilization_pct: number;
  status: "normal" | "warning" | "breached";
}

export interface RiskDashboardState {
  organization_id: string;
  kill_switch_active: boolean;
  kill_switch_activated_at?: string;
  kill_switch_actor?: string;
  max_drawdown_limit_bps: number;
  current_drawdown_bps: number;
  daily_loss_limit_usd_cents: number;
  current_daily_loss_cents: number;
  rules: RiskLimitRule[];
  as_of: string;
}

export interface ToggleKillSwitchInput {
  active: boolean;
  reason: string;
}

/** Fetches overall risk posture and effective limits. */
export async function getRiskDashboard(): Promise<RiskDashboardState> {
  return request<RiskDashboardState>("/api/saas/risk-dashboard");
}

/** Activates or deactivates the tenant-level emergency kill-switch. */
export async function toggleKillSwitch(
  input: ToggleKillSwitchInput,
): Promise<{ success: boolean; kill_switch_active: boolean; updated_at: string }> {
  return request<{ success: boolean; kill_switch_active: boolean; updated_at: string }>(
    "/api/saas/risk-dashboard/kill-switch",
    {
      method: "POST",
      body: input,
    },
  );
}
