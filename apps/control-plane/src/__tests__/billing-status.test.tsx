// @vitest-environment jsdom
/**
 * Billing status / reconcile / usage limits behaviour (Part 04, L3).
 *
 * The API client is mocked; reconcile must POST to the exact path, must not
 * POST when the operator declines confirmation, and the displayed result must
 * come from the server refetch of /api/saas/billing/reconcile/{id}.
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const status = vi.fn();
const reconcile = vi.fn();
const reconcileResult = vi.fn();
const commercialState = vi.fn();
const usageLimits = vi.fn();

vi.mock("@/lib/api/billing-status-api", () => ({
  billingStatusApi: {
    status: (...args: unknown[]) => status(...args),
    reconcile: (...args: unknown[]) => reconcile(...args),
    reconcileResult: (...args: unknown[]) => reconcileResult(...args),
    commercialState: (...args: unknown[]) => commercialState(...args),
    usageLimits: (...args: unknown[]) => usageLimits(...args),
  },
}));

vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", slug: "org-1", name: "Org One", status: "active", role: "owner", membership_status: "active" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

import BillingPage from "../app/billing/page";

const billingStatus = {
  organization_id: "org-1",
  plan_code: "pro",
  plan_version: 3,
  subscription_status: "active",
  billing_provider: "stripe",
  payment_state: null,
  invoice_state: null,
  entitlements_active: true,
  usage: { period: "2026-10", total_requests: 1240, total_trades: 86 },
  dunning_state: "current",
  grace_until: null,
  suspension_reason: null,
  as_of: "2026-10-20T08:00:00Z",
};

describe("billing status", () => {
  beforeEach(() => {
    status.mockReset();
    reconcile.mockReset();
    reconcileResult.mockReset();
    commercialState.mockReset();
    usageLimits.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("renders server billing status", async () => {
    status.mockResolvedValue(billingStatus);
    reconcileResult.mockResolvedValue(null as never);
    render(<BillingPage />);
    expect(await screen.findByText("PRO")).toBeTruthy();
    expect(await screen.findByText("Entitlements Active")).toBeTruthy();
    expect(status).toHaveBeenCalledWith("org-1");
  });

  it("reconcile posts exactly once after confirmation, then refetches the result", async () => {
    status.mockResolvedValue(billingStatus);
    reconcile.mockResolvedValue({
      organization_id: "org-1",
      action: "update",
      reason: "Provider entitlement aligned with local state",
      decided_at: "2026-10-20T09:00:00Z",
      idempotent: true,
      internal_subscription_status: "active",
      provider_event_kind: "subscription_updated",
    });
    reconcileResult.mockResolvedValue({
      organization_id: "org-1",
      action: "update",
      reason: "Provider entitlement aligned with local state",
      decided_at: "2026-10-20T09:00:00Z",
      idempotent: true,
      internal_subscription_status: "active",
      provider_event_kind: "subscription_updated",
    });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    render(<BillingPage />);
    await screen.findByText("PRO");

    fireEvent.click(screen.getByRole("button", { name: "Reconcile with Provider" }));
    await waitFor(() => expect(reconcile).toHaveBeenCalledTimes(1));
    expect(reconcile).toHaveBeenCalledWith();
    await waitFor(() => expect(reconcileResult).toHaveBeenCalledWith("org-1"));
    expect(await screen.findByText("update")).toBeTruthy();
    expect(await screen.findByText("Provider entitlement aligned with local state")).toBeTruthy();
    window.confirm = originalConfirm;
  });

  it("reconcile sends nothing when the operator declines", async () => {
    status.mockResolvedValue(billingStatus);
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => false);
    render(<BillingPage />);
    await screen.findByText("PRO");
    fireEvent.click(screen.getByRole("button", { name: "Reconcile with Provider" }));
    expect(reconcile).not.toHaveBeenCalled();
    window.confirm = originalConfirm;
  });

  it("renders usage limits from the server", async () => {
    status.mockResolvedValue(billingStatus);
    reconcileResult.mockResolvedValue(null as never);
    usageLimits.mockResolvedValue({
      organization_id: "org-1",
      period: "2026-10",
      plan_code: "pro",
      limits: [
        { feature: "api_requests", state: "within_allowance", limit: 10000, current: 1240, remaining: 8760, allows: true },
        { feature: "trades_per_month", state: "soft_limit_exceeded", limit: 100, current: 105, remaining: null, allows: true },
      ],
      as_of: "2026-10-20T08:00:00Z",
    });
    render(<BillingPage />);
    expect(await screen.findByText("api_requests")).toBeTruthy();
    expect(await screen.findByText("soft_limit_exceeded")).toBeTruthy();
    expect(usageLimits).toHaveBeenCalledWith("org-1");
  });
});
