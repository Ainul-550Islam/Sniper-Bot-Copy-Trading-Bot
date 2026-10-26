/**
 * Billing status, reconcile and usage-limit API client (Part 04, L3).
 *
 * Covers `GET /api/saas/billing/status/{id}`,
 * `POST /api/saas/billing/reconcile`, `GET /api/saas/billing/reconcile/{id}`,
 * `GET /api/saas/commercial/state/{id}` and `GET /api/saas/usage/limits/{id}`
 * — endpoints with OpenAPI contracts but no dedicated frontend panels.
 *
 * The reconcile trigger is gated to the roles the server permits; after a
 * successful POST the page refetches `/api/saas/billing/reconcile/{id}` and
 * renders the server-confirmed result — no optimistic status, no polling.
 */

import { request } from "../api";

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

export interface ReconcileResponse {
  organization_id: string;
  action: string;
  reason: string;
  decided_at: string;
  idempotent: boolean;
  internal_subscription_status: string | null;
  provider_event_kind: string | null;
}

export interface ReconcileRequestBody {
  dry_run?: boolean;
  event_kind?: string;
  provider?: string;
  provider_event_id?: string;
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

export interface UsageLimit {
  feature: string;
  state: string;
  limit: number | null;
  current: number;
  remaining: number | null;
  allows: boolean;
}

export interface UsageLimits {
  organization_id: string;
  period: string;
  plan_code: string;
  limits: UsageLimit[];
  as_of: string;
}

export const billingStatusApi = {
  status: (organizationId: string) =>
    request<BillingStatus>(
      `/api/saas/billing/status/${encodeURIComponent(organizationId)}`,
    ),

  /** Trigger reconciliation for the authenticated tenant's organization. */
  reconcile: () =>
    request<ReconcileResponse>("/api/saas/billing/reconcile", {
      method: "POST",
      body: { dry_run: false },
    }),

  /** Server-confirmed result of the last reconciliation for the organization. */
  reconcileResult: (organizationId: string) =>
    request<ReconcileResponse>(
      `/api/saas/billing/reconcile/${encodeURIComponent(organizationId)}`,
    ),

  commercialState: (organizationId: string) =>
    request<CommercialState>(
      `/api/saas/commercial/state/${encodeURIComponent(organizationId)}`,
    ),

  usageLimits: (organizationId: string) =>
    request<UsageLimits>(
      `/api/saas/usage/limits/${encodeURIComponent(organizationId)}`,
    ),
};
