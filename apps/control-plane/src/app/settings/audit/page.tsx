"use client";

/**
 * Immutable Audit Log Console (SECOND.md §62).
 *
 * Real-time inspection of tenant audit records: authentication events,
 * strategy parameter changes, credential rotations, and risk overrides.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { request } from "@/lib/api";

interface AuditEvent {
  id: string;
  actor: string;
  action: string;
  target?: string | null;
  created_at: string;
}

interface AuditExportRecord {
  id: string;
  actor: string;
  action: string;
  target?: string | null;
  at: string;
}

interface AuditExportResponse {
  data: {
    records: AuditExportRecord[];
  };
}

export default function AuditLogPage() {
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadLogs = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<AuditExportResponse>("/api/saas/exports?kind=audit");
      setEvents(
        res.data.records.map((record) => ({
          id: record.id,
          actor: record.actor,
          action: record.action,
          target: record.target,
          created_at: record.at,
        })),
      );
    } catch (err: unknown) {
      setEvents([]);
      setError(err instanceof Error ? err.message : "Failed to load audit ledger");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadLogs();
  }, [loadLogs]);

  return (
    <AppShell title="Audit Logs">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Compliance &amp; Security Audit Logs</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Append-only tamper-evident ledger tracking administrative actions, configuration changes, and bot controls.
          </p>
        </div>
        <button onClick={() => void loadLogs()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Ledger
        </button>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        {loading ? (
          <div style={{ padding: "2rem", textAlign: "center" }}>Loading audit ledger...</div>
        ) : (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Timestamp</th>
                <th style={{ padding: "0.75rem 1rem" }}>Actor</th>
                <th style={{ padding: "0.75rem 1rem" }}>Action Event</th>
                <th style={{ padding: "0.75rem 1rem" }}>Target / Parameters</th>
              </tr>
            </thead>
            <tbody>
              {events.length === 0 ? (
                <tr>
                  <td colSpan={4} style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
                    No audit events were returned for this tenant.
                  </td>
                </tr>
              ) : events.map((e) => (
                <tr key={e.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {new Date(e.created_at).toLocaleString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <code style={{ fontSize: "0.8rem" }}>{e.actor}</code>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className="badge" style={{ background: "var(--accent-glow)", color: "var(--accent)" }}>
                      {e.action}
                    </span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                    {e.target ? <code>{e.target}</code> : "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </AppShell>
  );
}
