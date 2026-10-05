/**
 * Typed Alerts & Incident Center Client (THIRD.md §128).
 *
 * Interfaces with `/api/saas/alerts`.
 */

import { request } from "../api";

export type AlertSeverity = "info" | "warning" | "critical" | "emergency";
export type AlertCategory = "risk" | "execution" | "billing" | "security" | "custody" | "system";

export interface AlertItem {
  id: string;
  organization_id: string;
  severity: AlertSeverity;
  category: AlertCategory;
  title: string;
  message: string;
  is_acknowledged: boolean;
  acknowledged_at?: string;
  acknowledged_by?: string;
  created_at: string;
}

export interface AlertsResponse {
  organization_id: string;
  items: AlertItem[];
  unacknowledged_count: number;
}

/** Lists tenant alerts with optional severity and acknowledgement filters. */
export async function listAlerts(options?: { severity?: string; unreadOnly?: boolean }): Promise<AlertsResponse> {
  const params = new URLSearchParams();
  if (options?.severity) params.set("severity", options.severity);
  if (options?.unreadOnly) params.set("unread", "true");
  const qs = params.toString() ? `?${params.toString()}` : "";
  return request<AlertsResponse>(`/api/saas/alerts${qs}`);
}

/** Acknowledges an alert by unique identifier. */
export async function acknowledgeAlert(
  id: string,
): Promise<{ success: boolean; id: string; acknowledged_at: string }> {
  return request<{ success: boolean; id: string; acknowledged_at: string }>(
    `/api/saas/alerts/${encodeURIComponent(id)}/ack`,
    {
      method: "POST",
    },
  );
}
