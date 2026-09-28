/**
 * Shared frontend API client for billing/commercial/usage/custody lifecycle (Batch 3).
 * Typed requests/responses, central error handling, authentication via lib/api headers.
 * No credentials in query strings.
 */
import { request, ApiError } from "@/lib/api";

export interface BillingStatus {
  organization_id: string;
  plan_code: string;
  plan_version: number;
  subscription_status: string;
  billing_provider: string;
  payment_state: string | null;
  invoice_state: string | null;
  entitlements_active: boolean;
  usage: { period: string; total_requests: number; total_trades: number };
  dunning_state: string;
  grace_until: string | null;
  suspension_reason: string | null;
  as_of: string;
}

export interface UsageLimits {
  organization_id: string;
  period: string;
  plan_code: string;
  limits: Array<{
    feature: string;
    state: string;
    limit: number | null;
    current: number;
    remaining: number | null;
    allows: boolean;
  }>;
  as_of: string;
}

export interface CommercialState {
  organization_id: string;
  plan_code: string;
  subscription_status: string;
  billing_provider: string;
  entitlements_active: boolean;
  dunning_state: string;
  lifecycle_status: string;
  commercial_consistent: boolean;
  as_of: string;
}

export interface CustodyHealth {
  organization_id: string;
  providers: Array<{ provider_type: string; state: string; detail: string; signing_allowed: boolean }>;
}

export interface LifecycleStatus {
  organization_id: string;
  organization_status: string;
  phase: string;
  custody_revoked: boolean;
  sessions_invalidated: boolean;
  retention_scheduled: boolean;
  purge_eligible_at: string | null;
}

export const commercial = {
  billingStatus: () => request<BillingStatus>("/api/saas/billing/status"),
  usageLimits: () => request<UsageLimits>("/api/saas/usage/limits"),
  commercialState: () => request<CommercialState>("/api/saas/commercial/state"),
  custodyHealth: () => request<CustodyHealth>("/api/saas/custody/health"),
  lifecycleStatus: (organizationId: string) =>
    request<LifecycleStatus>(`/api/saas/data-lifecycle/${encodeURIComponent(organizationId)}/status`),
  // Rotation helpers
  createRotation: (oldSignerId: string, newSignerId: string) =>
    request<{ id: string }>(`/api/saas/custody/rotations`, {
      method: "POST",
      body: { old_signer_id: oldSignerId, new_signer_id: newSignerId },
    }),
};

export function isApiError(e: unknown): e is ApiError {
  return e instanceof ApiError;
}

export function toDisplayError(e: unknown): string {
  if (e instanceof ApiError) return `${e.kind}: ${e.reason}`;
  if (e instanceof Error) return e.message;
  return "request failed";
}
