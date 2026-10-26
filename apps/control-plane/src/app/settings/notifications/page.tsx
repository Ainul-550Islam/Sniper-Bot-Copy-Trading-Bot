"use client";

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { notificationsApi, type NotificationsPreferences } from "@/lib/api/notifications-api";
import { toDisplayError } from "@/lib/api";

/**
 * Notification Preferences (Part 04, L5).
 *
 * Read-only: the OpenAPI contract exposes only `GET
 * /api/saas/notifications/preferences`, so the page renders server truth and
 * offers no write path.
 */
export default function NotificationsPage() {
  const [preferences, setPreferences] = useState<NotificationsPreferences | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setPreferences(await notificationsApi.preferences());
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Notification Preferences">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Notification Preferences</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Delivery channels for organization alerts. These preferences are read
          from the server; this console does not expose a write path for them.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading notification preferences…</div>
      ) : preferences ? (
        <div className="card" style={{ padding: 0, overflow: "hidden" }}>
          <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
            <h3 style={{ margin: 0 }}>{preferences.items.length} Channel{preferences.items.length !== 1 ? "s" : ""}</h3>
          </div>
          {preferences.items.length === 0 && (
            <p style={{ padding: "1rem", color: "var(--muted)" }}>
              No notification preferences were returned for this organization.
            </p>
          )}
          {preferences.items.length > 0 && (
            <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                  <th style={{ padding: "0.75rem 1rem" }}>Channel</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Enabled</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Detail</th>
                </tr>
              </thead>
              <tbody>
                {preferences.items.map((item) => (
                  <tr key={item.channel} style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>{item.channel}</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <span className={`badge ${item.enabled ? "badge-ok" : ""}`}>
                        {String(item.enabled)}
                      </span>
                    </td>
                    <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{item.detail ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
            As of {new Date(preferences.updated_at).toLocaleString()}. No client-side
            write path exists for this resource in the server contract.
          </p>
        </div>
      ) : null}
    </AppShell>
  );
}
