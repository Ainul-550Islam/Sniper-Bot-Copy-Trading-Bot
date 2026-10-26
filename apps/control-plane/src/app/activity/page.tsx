"use client";

/**
 * Customer Activity Timeline Page (THIRD.md §108).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { request } from "@/lib/api";
import { eventsApi, type OrganizationEvent } from "@/lib/api/events-api";

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

  const [events, setEvents] = useState<OrganizationEvent[]>([]);
  const [eventsLoading, setEventsLoading] = useState(false);
  const [eventsError, setEventsError] = useState<string | null>(null);

  const loadEvents = useCallback(async () => {
    setEventsLoading(true);
    setEventsError(null);
    try {
      const res = await eventsApi.list();
      setEvents(Array.isArray(res.events) ? res.events : []);
    } catch (err: unknown) {
      setEvents([]);
      setEventsError(err instanceof Error ? err.message : "Failed to load events feed");
    } finally {
      setEventsLoading(false);
    }
  }, []);

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
    void Promise.resolve().then(() => {
      void loadData();
      void loadEvents();
    });
  }, [loadData, loadEvents]);

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

      {/* Events feed from the organization event stream */}
      <section className="card" style={{ marginTop: "1.5rem" }} aria-label="Organization events feed">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <div>
            <h3 style={{ margin: 0 }}>Events Feed</h3>
            <p style={{ margin: "0.25rem 0 0", fontSize: "0.85rem", color: "var(--muted)" }}>
              Server-sent organization events (authentications, configuration
              changes, member actions, billing events).
            </p>
          </div>
          <button onClick={() => void loadEvents()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
            Refresh Feed
          </button>
        </div>
        {eventsLoading ? (
          <div style={{ padding: "1rem", color: "var(--muted)" }}>Loading events feed…</div>
        ) : eventsError ? (
          <div style={{ padding: "1rem", color: "var(--bad)" }}>{eventsError}</div>
        ) : (
          <div style={{ padding: 0, overflow: "hidden" }}>
            {events.length === 0 && (
              <p style={{ padding: "1rem", color: "var(--muted)" }}>
                No organization events were returned.
              </p>
            )}
            {events.length > 0 && (
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
                  {events.map((item) => (
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
                      <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>{item.summary}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}
      </section>
    </AppShell>
  );
}
