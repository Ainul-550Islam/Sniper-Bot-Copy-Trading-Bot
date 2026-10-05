"use client";

/**
 * Commercial Pricing & Plan Comparison (THIRD.md §111).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { formatUsdCents } from "@/lib/formatters/financial";
import { request } from "@/lib/api";
import Link from "next/link";

interface PlanTierItem {
  code: string;
  name: string;
  price_monthly_usd_cents: number;
  description: string;
  features: string[];
  is_popular?: boolean;
}

export default function PricingPage() {
  const [plans, setPlans] = useState<PlanTierItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadPlans = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<{ plans: PlanTierItem[] }>("/api/saas/pricing");
      setPlans(res.plans || []);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load pricing plans");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadPlans();
  }, [loadPlans]);

  return (
    <AppShell title="Subscription Plans">
      <div style={{ marginBottom: "2rem", textAlign: "center" }}>
        <h1 style={{ margin: 0 }}>Institutional Tier Plans</h1>
        <p style={{ margin: "0.5rem 0 0", color: "var(--muted)", fontSize: "0.95rem" }}>
          Deterministic execution scale, low-latency relayers, and FIPS 140-3 KMS signing.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadPlans()} />}

      {loading ? (
        <div className="card">Loading subscription plans...</div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))", gap: "1.5rem" }}>
          {plans.map((p) => (
            <div
              key={p.code}
              className="card"
              style={{
                borderColor: p.is_popular ? "var(--accent)" : "var(--line)",
                position: "relative",
                display: "flex",
                flexDirection: "column",
                justifyContent: "space-between",
              }}
            >
              {p.is_popular && (
                <span
                  className="badge"
                  style={{
                    position: "absolute",
                    top: "12px",
                    right: "12px",
                    background: "var(--accent)",
                    color: "#fff",
                  }}
                >
                  MOST POPULAR
                </span>
              )}

              <div>
                <h3 style={{ margin: "0 0 0.5rem", fontSize: "1.2rem" }}>{p.name}</h3>
                <div style={{ fontSize: "2rem", fontWeight: 700, fontFamily: "var(--mono)", margin: "0.5rem 0" }}>
                  {formatUsdCents(p.price_monthly_usd_cents)}
                  <span style={{ fontSize: "0.85rem", color: "var(--muted)", fontWeight: 400 }}> / month</span>
                </div>
                <p style={{ color: "var(--muted)", fontSize: "0.85rem", marginBottom: "1.5rem" }}>
                  {p.description}
                </p>

                <div style={{ borderTop: "1px solid var(--line)", paddingTop: "1rem", marginBottom: "1.5rem" }}>
                  <div style={{ fontSize: "0.8rem", fontWeight: 600, color: "var(--muted)", marginBottom: "0.5rem" }}>
                    Included Features:
                  </div>
                  <ul style={{ paddingLeft: "1.2rem", margin: 0, fontSize: "0.85rem", lineHeight: 1.8 }}>
                    {p.features.map((feat) => (
                      <li key={feat}>{feat}</li>
                    ))}
                  </ul>
                </div>
              </div>

              <Link href="/billing" className={`btn ${p.is_popular ? "btn-primary" : "btn-secondary"}`} style={{ textAlign: "center", display: "block" }}>
                Select {p.name}
              </Link>
            </div>
          ))}
        </div>
      )}
    </AppShell>
  );
}
