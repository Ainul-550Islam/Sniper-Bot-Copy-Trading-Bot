/**
 * Shared frontend API client for billing/commercial/usage/custody lifecycle (Batch 3 & Commercial Readiness).
 * Typed requests/responses, central error handling, authentication via lib/api headers.
 * No credentials in query strings. No client-side authorization assumptions.
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
  providers: Array<{
    provider_type: string;
    state: string;
    detail: string;
    signing_allowed: boolean;
  }>;
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

export interface InvoiceLineItem {
  description: string;
  amount_cents: number;
  currency: string;
  period_start: string | null;
  period_end: string | null;
}

export interface InvoiceRecord {
  id: string;
  organization_id: string;
  provider_invoice_id: string | null;
  provider: string;
  status: "draft" | "open" | "paid" | "void" | "uncollectible";
  amount_due_cents: number;
  amount_paid_cents: number;
  currency: string;
  created_at: string;
  due_date: string | null;
  paid_at: string | null;
  hosted_invoice_url: string | null;
  pdf_url: string | null;
  lines: InvoiceLineItem[];
}

export interface InvoicesResponse {
  organization_id: string;
  invoices: InvoiceRecord[];
  count: number;
}

export interface PlanTier {
  code: string;
  name: string;
  price_monthly_usd: number;
  description: string;
  features: string[];
  limits: {
    max_bots: number;
    max_orders_per_minute: number;
    supported_modules: string[];
    priority_support: boolean;
    dedicated_rpc: boolean;
  };
}

export interface CheckoutSessionCreated {
  id: string;
  organization_id: string;
  plan_code: string;
  provider: string;
  status: string;
  checkout_url: string | null;
  instructions: string | null;
  expires_at: string | null;
}

export interface OrganizationMember {
  membership_id: string;
  user: {
    id: string;
    email: string;
    display_name: string;
    status: string;
    created_at: string;
    last_login_at: string | null;
  } | null;
  role: "org_owner" | "admin" | "trader" | "viewer" | "auditor";
  status: "active" | "invited" | "suspended";
  created_at: string;
}

export interface TeamListResponse {
  count: number;
  members: OrganizationMember[];
}

export interface UserSessionRecord {
  id: string;
  prefix: string;
  created_at: string;
  expires_at: string;
  ip_address: string | null;
  user_agent: string | null;
  is_current: boolean;
}

export interface SecuritySummary {
  mfa_enforced: boolean;
  active_sessions_count: number;
  api_keys_count: number;
  last_password_change_at: string | null;
  recent_security_events: Array<{
    id: string;
    action: string;
    actor: string;
    timestamp: string;
    ip_address: string | null;
    success: boolean;
  }>;
}

export const AVAILABLE_PLANS: PlanTier[] = [
  {
    code: "starter",
    name: "Starter Trader",
    price_monthly_usd: 99,
    description: "Core Solana AMM launch monitoring and manual sniping automation.",
    features: [
      "Solana AMM Raydium / Pump.fun launch detection",
      "Standard sub-second routing",
      "Telegram notification alerts",
      "Paper trading simulation",
      "Standard custody support",
    ],
    limits: {
      max_bots: 2,
      max_orders_per_minute: 30,
      supported_modules: ["sniper", "telegram"],
      priority_support: false,
      dedicated_rpc: false,
    },
  },
  {
    code: "pro",
    name: "Professional Alpha",
    price_monthly_usd: 299,
    description: "High-frequency copy trading and automated execution with multi-wallet support.",
    features: [
      "Everything in Starter",
      "Real-time Copy Trading with wallet mirroring",
      "Polymarket V3 prediction market execution",
      "Advanced risk limits and stop-loss trailing",
      "Full backtesting suite with historical logs",
      "Multi-user team access (up to 5 seats)",
    ],
    limits: {
      max_bots: 10,
      max_orders_per_minute: 120,
      supported_modules: ["sniper", "copy", "polymarket", "telegram"],
      priority_support: true,
      dedicated_rpc: false,
    },
  },
  {
    code: "business",
    name: "Enterprise Business",
    price_monthly_usd: 799,
    description: "Institutional infrastructure with dedicated Geyser feeds and KMS custody.",
    features: [
      "Everything in Pro",
      "Dedicated Jito / Yellowstone Geyser RPC feeds",
      "AWS KMS & HashiCorp Vault hardware signing",
      "Zero-latency multi-region HA execution clusters",
      "Custom strategy builder with automated backtesting",
      "Audit trail export & SOC2-ready compliance reporting",
      "Unlimited team seats & dedicated account manager",
    ],
    limits: {
      max_bots: 50,
      max_orders_per_minute: 600,
      supported_modules: ["sniper", "copy", "polymarket", "telegram"],
      priority_support: true,
      dedicated_rpc: true,
    },
  },
];

export const commercial = {
  // Billing status & commercial state
  billingStatus: () => request<BillingStatus>("/api/saas/billing/status"),
  usageLimits: () => request<UsageLimits>("/api/saas/usage/limits"),
  commercialState: () => request<CommercialState>("/api/saas/commercial/state"),
  custodyHealth: () => request<CustodyHealth>("/api/saas/custody/health"),
  lifecycleStatus: (organizationId: string) =>
    request<LifecycleStatus>(`/api/saas/data-lifecycle/${encodeURIComponent(organizationId)}/status`),

  // Invoices & Checkout
  invoices: () => request<InvoicesResponse>("/api/saas/invoices"),
  invoiceDetail: (id: string) => request<InvoiceRecord>(`/api/saas/invoices/${encodeURIComponent(id)}`),
  createCheckout: (
    planCode: string,
    idempotencyKey: string,
    provider: "stripe" | "paddle" | "manual" = "manual",
    successUrl?: string,
    cancelUrl?: string,
  ) =>
    request<CheckoutSessionCreated>("/api/saas/checkout", {
      method: "POST",
      body: {
        plan_code: planCode,
        provider,
        idempotency_key: idempotencyKey,
        success_url: successUrl,
        cancel_url: cancelUrl,
      },
    }),

  // Team & Organization Members
  members: (organizationId: string) =>
    request<TeamListResponse>(`/api/saas/organizations/${encodeURIComponent(organizationId)}/members`),
  inviteMember: (organizationId: string, email: string, role: string) =>
    request<{ success: boolean; invitation_id: string }>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/members`,
      {
        method: "POST",
        body: { email, role },
      },
    ),
  removeMember: (organizationId: string, memberId: string) =>
    request<{ success: boolean }>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/members/${encodeURIComponent(memberId)}`,
      { method: "DELETE" },
    ),
  updateMemberRole: (organizationId: string, memberId: string, role: string) =>
    request<{ success: boolean }>(
      `/api/saas/organizations/${encodeURIComponent(organizationId)}/members/${encodeURIComponent(memberId)}`,
      {
        method: "PATCH",
        body: { role },
      },
    ),

  // Security & Sessions
  securitySummary: () => request<SecuritySummary>("/api/saas/security/summary").catch(() => ({
    mfa_enforced: false,
    active_sessions_count: 1,
    api_keys_count: 0,
    last_password_change_at: null,
    recent_security_events: [],
  })),

  // Custody Rotation
  createRotation: (profileId: string, oldSignerId: string, newSignerId: string) =>
    request<{ id: string }>(`/api/saas/custody/rotations`, {
      method: "POST",
      body: {
        profile_id: profileId,
        old_signer_id: oldSignerId,
        new_signer_id: newSignerId,
      },
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
