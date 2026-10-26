"use client";

import { useCallback, useEffect, useState } from "react";
import type { UsageLimits } from "@/lib/api/billing-status-api";
import { billingStatusApi } from "@/lib/api/billing-status-api";
import { toDisplayError } from "@/lib/api";

interface UsageLimitsPanelProps {
  organizationId: string;
}

/**
 * Usage limits panel: renders `GET /api/saas/usage/limits/{id}` feature by
 * feature. A `null` limit is "Unlimited"; a hard state is "Blocked". Nothing
 * is estimated client-side.
 */
export function UsageLimitsPanel({ organizationId }: UsageLimitsPanelProps) {
  const [limits, setLimits] = useState<UsageLimits | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const reported = await billingStatusApi.usageLimits(organizationId);
      // Server truth only: without a plan code and a limits array the panel
      // shows its honest empty state instead of crashing the page around it.
      setLimits(reported && typeof reported.plan_code === "string" && Array.isArray(reported.limits) ? reported : null);
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
    <section className="card" aria-label="Usage limits">
      <h2>Usage Quotas &amp; Tier Limits</h2>
      {error && (
        <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.75rem", borderRadius: "4px", marginBottom: "1rem" }}>
          {error}
        </div>
      )}
      {loading ? (
        <p>Loading usage limits…</p>
      ) : limits ? (
        <>
          <p className="muted small" style={{ marginTop: "0.25rem" }}>
            Plan <strong>{limits.plan_code}</strong> · Period{" "}
            <span style={{ whiteSpace: "nowrap" }}>{limits.period}</span> ·
            As of {new Date(limits.as_of).toLocaleString()}
          </p>
          {limits.limits.length === 0 ? (
            <p className="muted" style={{ marginTop: "1rem" }}>
              No feature limits configured for this tier.
            </p>
          ) : (
            <table className="table" style={{ marginTop: "1rem", width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                  <th style={{ padding: "0.75rem 1rem" }}>Feature</th>
                  <th style={{ padding: "0.75rem 1rem" }}>State</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Limit</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Current</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Remaining</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Allowed</th>
                </tr>
              </thead>
              <tbody>
                {limits.limits.map((l) => (
                  <tr key={l.feature} style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>{l.feature}</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <span className={`tag tag--${l.state === "within_allowance" ? "healthy" : l.state === "soft_limit_exceeded" ? "warning" : "error"}`}>{l.state}</span>
                    </td>
                    <td style={{ padding: "0.75rem 1rem" }}>{l.limit != null ? String(l.limit) : "Unlimited"}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{String(l.current)}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{l.remaining != null ? String(l.remaining) : "∞"}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <span className={l.allows ? "success" : "error"}>
                        {l.allows ? "✓ Yes" : "✕ Blocked"}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      ) : (
        <p className="muted">No usage limits were returned for this organization.</p>
      )}
    </section>
  );
}
