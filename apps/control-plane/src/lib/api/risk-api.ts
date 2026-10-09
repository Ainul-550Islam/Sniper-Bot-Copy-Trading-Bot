/**
 * Typed tenant risk limits and organization-scoped kill-switch client.
 *
 * Values come from the tenant configuration document. The client does not
 * convert non-USD values into fabricated dollar amounts.
 */

import { request } from "../api";

export interface RiskLimitRule {
  id: string;
  name: string;
  scope: string;
  limit_ref: number | null;
  current_utilization_ref: number | null;
  utilization_pct: number | null;
  status: "normal" | "warning" | "breached";
}

export interface RiskDashboardState {
  organization_id: string;
  kill_switch_active: boolean;
  reference_asset: string;
  max_drawdown_limit_ref: number | null;
  current_drawdown_ref: number | null;
  daily_loss_limit_ref: number | null;
  current_daily_loss_ref: number | null;
  durable: boolean;
  modules: Array<{ module: string; effective_state: "enabled" | "disabled" | "unknown"; override: unknown }>;
  rules: RiskLimitRule[];
  as_of: string;
}

export interface ToggleKillSwitchInput {
  active: boolean;
  reason: string;
}

export async function getRiskDashboard(): Promise<RiskDashboardState> {
  return request<RiskDashboardState>("/api/saas/risk-dashboard");
}

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
