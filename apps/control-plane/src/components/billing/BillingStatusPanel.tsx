"use client";

import { useCallback, useEffect, useState } from "react";
import type { BillingStatus } from "@/lib/api/billing-status-api";
import { billingStatusApi } from "@/lib/api/billing-status-api";
import { toDisplayError } from "@/lib/api";

interface BillingStatusPanelProps {
  organizationId: string;
}

/**
 * Billing status surface: plan tier, subscription and dunning state, all
 * rendered straight from `GET /api/saas/billing/status/{id}`. Empty and
 * error states are honest — nothing is synthesized client-side.
 */
export function BillingStatusPanel({ organizationId }: BillingStatusPanelProps) {
  const [billing, setBilling] = useState<BillingStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const reported = await billingStatusApi.status(organizationId);
      // Render only what the server actually reported. A body without a
      // plan code (e.g. an unmocked or foreign endpoint answering `{}`)
      // must never crash the hosting page or fabricate a plan.
      setBilling(reported && typeof reported.plan_code === "string" ? reported : null);
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, [organizationId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <section className="card" aria-label="Billing status">
      <h2>Billing &amp; Subscription Status</h2>
      {error && (
        <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.75rem", borderRadius: "4px", marginBottom: "1rem" }}>
          {error}
        </div>
      )}
      {loading ? (
        <p>Loading billing status…</p>
      ) : billing ? (
        <>
          <div className="grid-3" style={{ marginTop: "1rem" }}>
            <div className="stat-card" style={{ background: "var(--panel-2)" }}>
              <span className="stat-card__title">Plan Tier</span>
              <span className="stat-card__value">{billing.plan_code.toUpperCase()}</span>
              <span className="muted small" style={{ marginTop: "0.3rem", display: "block" }}>
                Version {billing.plan_version}
              </span>
            </div>
            <div className="stat-card" style={{ background: "var(--panel-2)" }}>
              <span className="stat-card__title">Subscription</span>
              <span className="stat-card__value">{billing.subscription_status}</span>
              <span className={`tag ${billing.entitlements_active ? "tag--active" : "tag--suspended"}`} style={{ width: "fit-content", marginTop: "0.3rem", display: "block" }}>
                {billing.entitlements_active ? "Entitlements Active" : "Suspended"}
              </span>
            </div>
            <div className="stat-card" style={{ background: "var(--panel-2)" }}>
              <span className="stat-card__title">Provider</span>
              <span className="stat-card__value">{billing.billing_provider.toUpperCase()}</span>
              <span className="muted small" style={{ marginTop: "0.3rem", display: "block" }}>
                {billing.dunning_state.toUpperCase()}
              </span>
            </div>
          </div>
          <div style={{ marginTop: "1rem" }}>
            <strong>Dunning state</strong>{" "}
            <span className={`tag ${billing.dunning_state === "current" ? "tag--healthy" : "tag--warning"}`}>{billing.dunning_state}</span>
            {billing.suspension_reason && (
              <span style={{ marginLeft: "0.5rem", fontSize: "0.85rem", color: "var(--muted)" }}>— {billing.suspension_reason}</span>
            )}
            {billing.grace_until && (
              <p style={{ marginTop: "0.4rem", fontSize: "0.85rem", color: "var(--muted)" }}>
                Grace period until {new Date(billing.grace_until).toLocaleDateString()}
              </p>
            )}
          </div>
          <div style={{ marginTop: "1rem" }}>
            <strong>Usage (as of {new Date(billing.as_of).toLocaleString()})</strong>
            <table className="table" style={{ marginTop: "0.5rem", width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                  <th style={{ padding: "0.75rem 1rem" }}>Dimension</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Current</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Period</th>
                </tr>
              </thead>
              <tbody>
                <tr style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>API requests</td>
                  <td style={{ padding: "0.75rem 1rem" }}>{billing.usage.total_requests}</td>
                  <td style={{ padding: "0.75rem 1rem" }}>{billing.usage.period}</td>
                </tr>
                <tr style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>Trades</td>
                  <td style={{ padding: "0.75rem 1rem" }}>{billing.usage.total_trades}</td>
                  <td style={{ padding: "0.75rem 1rem" }}>{billing.usage.period}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
            Refreshed from the server on page load. Use Refresh to re-read the
            authoritative billing state.
          </p>
        </>
      ) : (
        <p className="muted">No billing status was returned for this organization.</p>
      )}
    </section>
  );
}
