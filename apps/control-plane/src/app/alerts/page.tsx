"use client";

/**
 * Customer Alerts Center Page (THIRD.md §107).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import AlertCenter from "@/components/alerts/AlertCenter";
import { ErrorState } from "@/components/common/ErrorState";
import { AlertItem, listAlerts } from "@/lib/api/alerts-api";

export default function AlertsPage() {
  const [alerts, setAlerts] = useState<AlertItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadAlerts = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await listAlerts();
      setAlerts(res.items || []);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load alerts");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadAlerts());
  }, [loadAlerts]);

  return (
    <AppShell title="Alerts Center">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Incident &amp; Risk Alerts Center</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Real-time execution anomaly detection, slippage warnings, and security notifications.
          </p>
        </div>
        <button onClick={() => void loadAlerts()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Alerts
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadAlerts()} />}

      {loading ? (
        <div className="card">Loading alerts...</div>
      ) : (
        <AlertCenter alerts={alerts} onRefresh={() => void loadAlerts()} />
      )}
    </AppShell>
  );
}
