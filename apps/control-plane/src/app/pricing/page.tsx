"use client";

/**
 * Commercial pricing and entitlement catalogue.
 *
 * Monetary prices are rendered only when the billing provider has supplied an
 * authoritative price snapshot. The page does not invent prices or security
 * claims when the provider is not configured.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { formatUsdCents } from "@/lib/formatters/financial";
import { request } from "@/lib/api";

interface PlanTierItem {
  code: string;
  name: string;
  price_monthly_usd_cents: number | null;
  price_yearly_usd_cents: number | null;
  prices_available: boolean;
  description: string;
  features: string[];
  status: string;
}

export default function PricingPage() {
  const [plans, setPlans] = useState<PlanTierItem[]>([]);
  const [pricingStatus, setPricingStatus] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadPlans = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<{ plans: PlanTierItem[]; pricing_status?: string }>("/api/saas/pricing");
      setPlans(res.plans || []);
      setPricingStatus(res.pricing_status ?? null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load pricing plans");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadPlans());
  }, [loadPlans]);

  return (
    <AppShell title="Subscription Plans">
      <div style={{ marginBottom: "2rem", textAlign: "center" }}>
        <h1 style={{ margin: 0 }}>Subscription Plans</h1>
        <p style={{ margin: "0.5rem 0 0", color: "var(--muted)", fontSize: "0.95rem" }}>
          Current plan entitlements and provider-backed billing availability.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadPlans()} />}

      {loading ? (
        <div className="card">Loading subscription plans...</div>
      ) : (
        <>
          {pricingStatus && pricingStatus !== "active" && (
            <div className="card" style={{ marginBottom: "1.5rem", color: "var(--muted)" }}>
              Monetary pricing is not currently available from the configured billing provider. Plan entitlements
              remain visible, but checkout cannot be started from this page.
            </div>
          )}
          {plans.length === 0 ? (
            <div className="card">No public plans are available from the authoritative catalogue.</div>
          ) : (
            <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))", gap: "1.5rem" }}>
              {plans.map((plan) => (
                <div key={plan.code} className="card" style={{ display: "flex", flexDirection: "column", justifyContent: "space-between" }}>
                  <div>
                    <h3 style={{ margin: "0 0 0.5rem", fontSize: "1.2rem" }}>{plan.name}</h3>
                    <div style={{ fontSize: "1.25rem", fontWeight: 700, fontFamily: "var(--mono)", margin: "0.5rem 0" }}>
                      {plan.prices_available && plan.price_monthly_usd_cents !== null
                        ? `${formatUsdCents(plan.price_monthly_usd_cents)} / month`
                        : "Price supplied at checkout"}
                    </div>
                    <p style={{ color: "var(--muted)", fontSize: "0.85rem", marginBottom: "1.5rem" }}>
                      {plan.description || "No plan description has been published."}
                    </p>

                    <div style={{ borderTop: "1px solid var(--line)", paddingTop: "1rem", marginBottom: "1.5rem" }}>
                      <div style={{ fontSize: "0.8rem", fontWeight: 600, color: "var(--muted)", marginBottom: "0.5rem" }}>
                        Entitlement keys:
                      </div>
                      {plan.features.length > 0 ? (
                        <ul style={{ paddingLeft: "1.2rem", margin: 0, fontSize: "0.85rem", lineHeight: 1.8 }}>
                          {plan.features.map((feature) => <li key={feature}>{feature}</li>)}
                        </ul>
                      ) : (
                        <p style={{ margin: 0, color: "var(--muted)", fontSize: "0.85rem" }}>No enabled features published.</p>
                      )}
                    </div>
                  </div>
                  <div style={{ color: "var(--muted)", fontSize: "0.8rem" }}>
                    Catalogue status: {plan.status}
                  </div>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </AppShell>
  );
}
