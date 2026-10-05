"use client";

/**
 * Customer Activity Timeline Page (THIRD.md §108).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { request } from "@/lib/api";

interface ActivityItem {
  id: string;
  actor: string;
  action: string;
  object_type: string;
  object_id: string;
  summary: string;
  timestamp: string;
}

export default function ActivityPage() {
  const [items, setItems] = useState<ActivityItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<{ items: ActivityItem[] }>("/api/saas/activity");
      setItems(res.items || []);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load activity feed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  return (
    <AppShell title="Activity Feed">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Organization Activity Timeline</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Audit trail of bot deployments, configuration edits, member actions, and billing events.
          </p>
        </div>
        <button onClick={() => void loadData()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Feed
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadData()} />}

      {loading ? (
        <div className="card">Loading activity timeline...</div>
      ) : (
        <div className="card" style={{ padding: 0, overflow: "hidden" }}>
          <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
            <h3 style={{ margin: 0 }}>Recent Organization Events ({items.length})</h3>
          </div>

          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Timestamp</th>
                <th style={{ padding: "0.75rem 1rem" }}>Actor</th>
                <th style={{ padding: "0.75rem 1rem" }}>Action</th>
                <th style={{ padding: "0.75rem 1rem" }}>Event Summary</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => (
                <tr key={item.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {new Date(item.timestamp).toLocaleString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <code style={{ fontSize: "0.8rem" }}>{item.actor}</code>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className="badge" style={{ background: "var(--accent-glow)", color: "var(--accent)" }}>
                      {item.action}
                    </span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                    {item.summary}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </AppShell>
  );
}
