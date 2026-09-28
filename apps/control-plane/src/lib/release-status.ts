/**
 * Frontend-safe release/status client model (Batch 3).
 * Consumes operator/public-safe readiness metadata only, never exposes secrets.
 * Never claims "production verified" based merely on frontend data.
 */

import { request } from "@/lib/api";

export interface PublicReadiness {
  ok: boolean;
  version: string;
  as_of: string;
  services: Record<string, string>;
}

export interface OperatorReadiness {
  ok: boolean;
  version: string;
  as_of: string;
  services: Record<string, string>;
  diagnostics: Record<string, unknown>;
}

export type DisplayStatus = "healthy" | "degraded" | "blocked" | "unknown";

export function mapToDisplay(services: Record<string, string>): DisplayStatus {
  const values = Object.values(services).map((v) => v.toLowerCase());
  if (values.some((v) => v.includes("blocked") || v.includes("mismatch"))) return "blocked";
  if (values.some((v) => v.includes("degraded") || v.includes("unreachable") || v.includes("not_configured"))) return "degraded";
  if (values.every((v) => v === "ok" || v === "healthy" || v === "ready")) return "healthy";
  return "unknown";
}

export const releaseStatus = {
  public: () => request<PublicReadiness>("/api/saas/readiness"),
  operator: () => request<OperatorReadiness>("/api/saas/readiness/operator"),
};

export function formatVersion(v: string): string {
  return v.trim() || "unknown";
}
