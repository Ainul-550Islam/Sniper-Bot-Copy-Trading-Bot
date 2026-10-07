"use client";

/**
 * Commercial Billing & Subscription Self-Service Desk (Commercial Readiness).
 *
 * Full self-service billing experience: plan comparison tiers, upgrade checkout creation,
 * invoice history with downloadable records, usage limits consumption, and dunning state.
 */

import { useEffect, useState, useCallback } from "react";
import { AppShell } from "@/components/AppShell";
import {
  commercial,
  toDisplayError,
  BillingStatus,
  UsageLimits,
  CommercialState,
  InvoiceRecord,
  PlanTier,
} from "@/lib/commercial";

export default function BillingPage() {
  const [billing, setBilling] = useState<BillingStatus | null>(null);
  const [usage, setUsage] = useState<UsageLimits | null>(null);
  const [, setComm] = useState<CommercialState | null>(null);
  const [invoices, setInvoices] = useState<InvoiceRecord[]>([]);
  const [plans, setPlans] = useState<PlanTier[]>([]);
  const [pricingStatus, setPricingStatus] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [checkoutNotice, setCheckoutNotice] = useState<string | null>(null);
  const [upgradingCode, setUpgradingCode] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [b, u, c, inv, p] = await Promise.all([
        commercial.billingStatus(),
        commercial.usageLimits(),
        commercial.commercialState(),
        commercial.invoices(),
        commercial.pricing(),
      ]);
      setBilling(b);
      setUsage(u);
      setComm(c);
      setInvoices(inv.invoices);
      setPlans(p.plans);
      setPricingStatus(p.pricing_status ?? null);
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  const handleUpgrade = useCallback(async (plan: PlanTier) => {
    setUpgradingCode(plan.code);
    setCheckoutNotice(null);
    setError(null);
    try {
      const idempotencyKey = "chk_upgrade_" + plan.code;
      const res = await commercial.createCheckout(plan.code, idempotencyKey, "manual");
      if (res.checkout_url && typeof window !== "undefined") {
        window.location.assign(res.checkout_url);
      } else {
        setCheckoutNotice(
          `Checkout session created for ${plan.name} (${res.id}). Provider instructions: ${res.instructions ?? "Contact account administrator to complete payment."}`,
        );
      }
      await loadData();
    } catch (err) {
      setError(toDisplayError(err));
    } finally {
      setUpgradingCode(null);
    }
  }, [loadData]);

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Subscription &amp; Commercial Billing</h1>
            <p className="muted">
              Manage your subscription tier, track real-time quota usage, review invoices, and upgrade features.
            </p>
          </div>
        </div>

        {error && <div className="notice danger">{error}</div>}
        {checkoutNotice && <div className="notice success">{checkoutNotice}</div>}

        {loading ? (
          <p className="muted">Loading subscription and usage status…</p>
        ) : (
          <>
            {/* Active Subscription Overview */}
            {billing && (
              <div className="grid-3">
                <div className="stat-card">
                  <span className="stat-card__title">Current Plan Tier</span>
                  <span className="stat-card__value">{billing.plan_code.toUpperCase()}</span>
                  <span className={`tag ${billing.entitlements_active ? "tag--active" : "tag--suspended"}`} style={{ width: "fit-content", marginTop: "0.4rem" }}>
                    {billing.entitlements_active ? "Entitlements Active" : "Suspended"}
                  </span>
                </div>

                <div className="stat-card">
                  <span className="stat-card__title">Billing Provider</span>
                  <span className="stat-card__value">{billing.billing_provider.toUpperCase()}</span>
                  <span className="muted small">Status: {billing.subscription_status}</span>
                </div>

                <div className="stat-card">
                  <span className="stat-card__title">Dunning State</span>
                  <span className="stat-card__value">{billing.dunning_state.toUpperCase()}</span>
                  <span className="muted small">
                    {billing.grace_until ? `Grace period until ${new Date(billing.grace_until).toLocaleDateString()}` : "Good Standing"}
                  </span>
                </div>
              </div>
            )}

            {/* Plan Comparison & Self-Service Upgrade */}
            <section className="card">
              <h2>Available Subscription Plans</h2>
              {pricingStatus && pricingStatus !== "active" && (
                <p className="muted small">Prices are not available from the configured billing provider; checkout may be refused.</p>
              )}
              <div className="grid-3" style={{ marginTop: "1rem" }}>
                {plans.length === 0 ? <p className="muted">No public plans are available.</p> : plans.map((plan) => {
                  const isCurrent = billing?.plan_code.toLowerCase() === plan.code.toLowerCase();
                  return (
                    <div
                      key={plan.code}
                      className="card stack"
                      style={{
                        background: "var(--panel-2)",
                        border: isCurrent ? "2px solid var(--accent)" : "1px solid var(--line)",
                      }}
                    >
                      <div className="row-between">
                        <h3>{plan.name}</h3>
                        {isCurrent && <span className="tag tag--active">Current Plan</span>}
                      </div>
                      <p style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.2rem 0" }}>
                        {plan.prices_available && plan.price_monthly_usd_cents !== null
                          ? `$${(plan.price_monthly_usd_cents / 100).toFixed(2)} / month`
                          : "Price supplied at checkout"}
                      </p>
                      <p className="muted small">{plan.description}</p>
                      <ul style={{ paddingLeft: "1.2rem", margin: "0.5rem 0" }}>
                        {plan.features.map((f, idx) => (
                          <li key={idx} className="small" style={{ marginBottom: "0.3rem" }}>
                            {f}
                          </li>
                        ))}
                      </ul>
                      <button
                        className={isCurrent ? "" : "primary"}
                        disabled={isCurrent || upgradingCode === plan.code}
                        onClick={() => handleUpgrade(plan)}
                        style={{ marginTop: "auto" }}
                      >
                        {isCurrent ? "Active Plan" : upgradingCode === plan.code ? "Initiating Checkout…" : `Upgrade to ${plan.name}`}
                      </button>
                    </div>
                  );
                })}
              </div>
            </section>

            {/* Usage Quotas & Limits */}
            {usage && (
              <section className="card">
                <h2>Usage Quotas &amp; Tier Limits ({usage.period})</h2>
                {usage.limits.length === 0 ? (
                  <p className="muted" style={{ marginTop: "0.5rem" }}>No feature limits configured for this tier.</p>
                ) : (
                  <div style={{ marginTop: "1rem", overflowX: "auto" }}>
                    <table>
                      <thead>
                        <tr>
                          <th>Feature / Dimension</th>
                          <th>Status</th>
                          <th>Current Usage / Tier Limit</th>
                          <th>Capacity Remaining</th>
                          <th>Access Allowed</th>
                        </tr>
                      </thead>
                      <tbody>
                        {usage.limits.map((l) => (
                          <tr key={l.feature}>
                            <td><strong>{l.feature}</strong></td>
                            <td>
                              <span className={`tag tag--${l.state === "ok" ? "healthy" : "warning"}`}>{l.state}</span>
                            </td>
                            <td>
                              {l.current} / {l.limit != null ? l.limit : "Unlimited"}
                            </td>
                            <td>{l.remaining != null ? `${l.remaining} units` : "∞"}</td>
                            <td>
                              <span className={l.allows ? "success" : "error"}>{l.allows ? "✓ Yes" : "✕ Blocked"}</span>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </section>
            )}

            {/* Invoice History */}
            <section className="card">
              <h2>Invoice History &amp; Receipts</h2>
              {invoices.length === 0 ? (
                <p className="muted" style={{ marginTop: "0.5rem" }}>
                  No invoices generated yet for this organization.
                </p>
              ) : (
                <div style={{ marginTop: "1rem", overflowX: "auto" }}>
                  <table>
                    <thead>
                      <tr>
                        <th>Invoice ID</th>
                        <th>Created Date</th>
                        <th>Amount</th>
                        <th>Status</th>
                        <th>Paid Date</th>
                        <th>Receipt</th>
                      </tr>
                    </thead>
                    <tbody>
                      {invoices.map((inv) => (
                        <tr key={inv.id}>
                          <td><code>{inv.id.slice(0, 12)}…</code></td>
                          <td>{new Date(inv.created_at).toLocaleDateString()}</td>
                          <td>
                            ${(inv.amount_due_cents / 100).toFixed(2)} {inv.currency.toUpperCase()}
                          </td>
                          <td>
                            <span className={`tag tag--${inv.status === "paid" ? "healthy" : "warning"}`}>
                              {inv.status}
                            </span>
                          </td>
                          <td>{inv.paid_at ? new Date(inv.paid_at).toLocaleDateString() : "—"}</td>
                          <td>
                            {inv.hosted_invoice_url ? (
                              <a href={inv.hosted_invoice_url} target="_blank" rel="noreferrer">
                                View Invoice ↗
                              </a>
                            ) : (
                              <span className="muted">System Receipt</span>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </AppShell>
  );
}
