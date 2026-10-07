"use client";

/**
 * Tenant infrastructure and integration status.
 *
 * Only health-registry evidence returned by the authenticated backend is shown.
 * Missing providers are not rendered as connected.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { toDisplayError } from "@/lib/api";
import { customerTrading, type IntegrationItem } from "@/lib/customer-trading-api";

function statusLabel(status: IntegrationItem["status"]): string {
  switch (status) {
    case "connected":
      return "CONNECTED";
    case "degraded":
      return "DEGRADED";
    case "error":
      return "ERROR";
    default:
      return "NOT CONFIGURED";
  }
}

export default function IntegrationsPage() {
  const [integrations, setIntegrations] = useState<IntegrationItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadIntegrations = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setIntegrations((await customerTrading.integrations()).items);
    } catch (err: unknown) {
      setIntegrations([]);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadIntegrations());
  }, [loadIntegrations]);

  return (
    <AppShell title="Ecosystem Integrations">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Connected Infrastructure &amp; Ecosystem Integrations</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Status is derived from the deployment health registry. A configured endpoint is not reported as live unless it has been sampled successfully.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          <div>{error}</div>
          <button type="button" onClick={() => void loadIntegrations()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
            Retry
          </button>
        </div>
      )}

      {loading ? (
        <div className="card">Loading integration evidence...</div>
      ) : integrations.length === 0 ? (
        <div className="card" style={{ color: "var(--muted)" }}>
          No integration health components are registered in this deployment.
        </div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))", gap: "1.25rem" }}>
          {integrations.map((item) => (
            <div key={item.id} className="card">
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "0.75rem" }}>
                <div>
                  <span style={{ fontSize: "0.75rem", color: "var(--muted)", textTransform: "uppercase" }}>
                    {item.category.replaceAll("_", " ")}
                  </span>
                  <h3 style={{ margin: "0.25rem 0 0", fontSize: "1.1rem" }}>{item.provider_name}</h3>
                </div>
                <span className={`badge ${item.status === "connected" ? "badge-ok" : item.status === "not_configured" ? "badge-warn" : "badge-bad"}`}>
                  {statusLabel(item.status)}
                </span>
              </div>

              <p style={{ fontSize: "0.85rem", color: "var(--muted)", margin: "0 0 1rem" }}>
                Evidence: {item.evidence_level.replaceAll("_", " ")}
              </p>

              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", fontSize: "0.8rem", borderTop: "1px solid var(--line)", paddingTop: "0.75rem" }}>
                <span style={{ color: item.status === "connected" ? "var(--ok)" : "var(--muted)", fontWeight: 600 }}>
                  {item.latency_ms === null ? "Latency unavailable" : `RTT: ${item.latency_ms}ms`}
                </span>
                <span style={{ color: "var(--muted)" }}>
                  {item.last_health_check_at ? new Date(item.last_health_check_at).toLocaleString() : "No sample timestamp"}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </AppShell>
  );
}
