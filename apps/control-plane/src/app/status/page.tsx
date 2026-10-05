"use client";

/**
 * Customer-Facing Service Status Page (THIRD.md §112).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import ServiceStatusGrid from "@/components/status/ServiceStatusGrid";
import { ErrorState } from "@/components/common/ErrorState";
import { ServiceStatusReport, getServiceStatus } from "@/lib/api/status-api";

export default function StatusPage() {
  const [report, setReport] = useState<ServiceStatusReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getServiceStatus();
      setReport(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load service status");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  return (
    <AppShell title="System Status">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>System Health &amp; Infrastructure Status</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Real-time telemetry on execution relayers, Geyser streams, KMS signers, and storage clusters.
          </p>
        </div>
        <button onClick={() => void loadStatus()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Telemetry
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadStatus()} />}

      {loading ? (
        <div className="card">Loading telemetry status...</div>
      ) : report ? (
        <div>
          <div
            className="card"
            style={{
              borderColor: report.overall_status === "operational" ? "var(--ok)" : "var(--warn)",
              background: report.overall_status === "operational" ? "var(--ok-glow)" : "var(--warn-glow)",
              marginBottom: "1.5rem",
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
            }}
          >
            <div>
              <h3 style={{ margin: 0, color: report.overall_status === "operational" ? "var(--ok)" : "var(--warn)" }}>
                All Systems Operational
              </h3>
              <p style={{ margin: "0.25rem 0 0", fontSize: "0.85rem" }}>
                Zero active degraded incidents across global relayer clusters.
              </p>
            </div>
            <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>
              As of {new Date(report.as_of).toLocaleTimeString()}
            </div>
          </div>

          <ServiceStatusGrid components={report.components} />
        </div>
      ) : null}
    </AppShell>
  );
}
