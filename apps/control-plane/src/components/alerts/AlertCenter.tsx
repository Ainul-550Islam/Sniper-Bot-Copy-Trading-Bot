"use client";

import { useState } from "react";
import { AlertItem, acknowledgeAlert } from "@/lib/api/alerts-api";

interface AlertCenterProps {
  alerts: AlertItem[];
  onRefresh: () => void;
}

export function AlertCenter({ alerts, onRefresh }: AlertCenterProps) {
  const [loadingId, setLoadingId] = useState<string | null>(null);

  const handleAck = async (id: string) => {
    setLoadingId(id);
    try {
      await acknowledgeAlert(id);
      onRefresh();
    } catch {
      // Ignore
    } finally {
      setLoadingId(null);
    }
  };

  if (alerts.length === 0) {
    return (
      <div className="card" style={{ textAlign: "center", padding: "2.5rem" }}>
        <p style={{ margin: 0, color: "var(--muted)" }}>No unacknowledged system or risk alerts.</p>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Incident &amp; Risk Alert Ledger ({alerts.length})</h3>
      </div>

      <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
            <th style={{ padding: "0.75rem 1rem" }}>Severity</th>
            <th style={{ padding: "0.75rem 1rem" }}>Category</th>
            <th style={{ padding: "0.75rem 1rem" }}>Title &amp; Message</th>
            <th style={{ padding: "0.75rem 1rem" }}>Timestamp</th>
            <th style={{ padding: "0.75rem 1rem" }}>Status</th>
          </tr>
        </thead>
        <tbody>
          {alerts.map((alt) => {
            const isCrit = alt.severity === "critical" || alt.severity === "emergency";
            const isWarn = alt.severity === "warning";
            return (
              <tr key={alt.id} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <span
                    className={`badge ${
                      isCrit ? "badge-bad" : isWarn ? "badge-warn" : "badge"
                    }`}
                  >
                    {alt.severity.toUpperCase()}
                  </span>
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <span className="badge" style={{ textTransform: "uppercase" }}>{alt.category}</span>
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <strong>{alt.title}</strong>
                  <div style={{ fontSize: "0.85rem", color: "var(--muted)", marginTop: "0.2rem" }}>
                    {alt.message}
                  </div>
                </td>
                <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                  {new Date(alt.created_at).toLocaleString()}
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  {alt.is_acknowledged ? (
                    <span className="badge badge-ok">ACKNOWLEDGED</span>
                  ) : (
                    <button
                      onClick={() => handleAck(alt.id)}
                      disabled={loadingId === alt.id}
                      className="btn btn-secondary"
                      style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem" }}
                    >
                      {loadingId === alt.id ? "Acknowledging..." : "Acknowledge"}
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default AlertCenter;
