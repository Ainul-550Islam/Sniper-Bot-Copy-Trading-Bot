/**
 * Typed Infrastructure & Service Status Client (THIRD.md §130).
 *
 * Interfaces with `/api/saas/status`.
 */

import { request } from "../api";

export type ComponentHealth = "operational" | "degraded" | "unavailable" | "maintenance";

export interface StatusComponent {
  name: string;
  category: string;
  status: ComponentHealth;
  latency_ms: number | null;
  last_checked: string | null;
  details: string;
}

export interface ServiceStatusReport {
  overall_status: ComponentHealth;
  as_of: string;
  components: StatusComponent[];
  active_incidents_count: number | null;
}

/** Fetches real-time status of trading infrastructure and external feeds. */
export async function getServiceStatus(): Promise<ServiceStatusReport> {
  return request<ServiceStatusReport>("/api/saas/status");
}
