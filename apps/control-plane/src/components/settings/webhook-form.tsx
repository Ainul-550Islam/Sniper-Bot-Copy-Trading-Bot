"use client";

import { useState } from "react";
import { WebhookEndpoint, createWebhook, deleteWebhook, testWebhook } from "@/lib/api/webhook-api";

interface WebhookFormProps {
  webhooks: WebhookEndpoint[];
  onRefresh: () => void;
}

export default function WebhookForm({ webhooks, onRefresh }: WebhookFormProps) {
  const [url, setUrl] = useState("");
  const [description, setDescription] = useState("");
  const [events, setEvents] = useState<string[]>(["trade.executed", "risk.limit_breached"]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);

  const availableEvents = [
    "trade.executed",
    "order.created",
    "order.cancelled",
    "risk.limit_breached",
    "circuit_breaker.tripped",
    "custody.rotated",
  ];

  const handleToggleEvent = (ev: string) => {
    if (events.includes(ev)) {
      setEvents(events.filter((e) => e !== ev));
    } else {
      setEvents([...events, ev]);
    }
  };

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);

    try {
      await createWebhook({
        url,
        description,
        events,
      });
      setUrl("");
      setDescription("");
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to register webhook");
    } finally {
      setLoading(false);
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm("Are you sure you want to remove this webhook endpoint?")) return;
    setLoading(true);
    try {
      await deleteWebhook(id);
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to delete webhook");
    } finally {
      setLoading(false);
    }
  };

  const handleTest = async (id: string) => {
    setLoading(true);
    setTestResult(null);
    try {
      const res = await testWebhook(id);
      setTestResult(`Test ping delivered successfully! HTTP ${res.status_code} in ${res.latency_ms}ms.`);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Test ping failed");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div>
      <div className="card" style={{ marginBottom: "1.5rem" }}>
        <h3 style={{ marginTop: 0 }}>Register Outbound Webhook</h3>
        {error && (
          <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
            {error}
          </div>
        )}
        {testResult && (
          <div style={{ color: "var(--ok)", background: "var(--ok-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
            {testResult}
          </div>
        )}

        <form onSubmit={handleCreate}>
          <div style={{ marginBottom: "1rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Endpoint HTTPS URL
            </label>
            <input
              type="url"
              required
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://your-domain.example/webhooks/bot-events"
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div style={{ marginBottom: "1rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Description
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Production risk alerting pipeline"
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div style={{ marginBottom: "1rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.5rem", color: "var(--muted)" }}>
              Subscribed Event Topics
            </label>
            <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
              {availableEvents.map((ev) => {
                const active = events.includes(ev);
                return (
                  <button
                    key={ev}
                    type="button"
                    onClick={() => handleToggleEvent(ev)}
                    className={`btn ${active ? "btn-primary" : "btn-secondary"}`}
                    style={{ fontSize: "0.8rem", padding: "0.25rem 0.5rem" }}
                  >
                    {ev}
                  </button>
                );
              })}
            </div>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary">
            {loading ? "Registering..." : "Create Webhook"}
          </button>
        </form>
      </div>

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <h3 style={{ margin: 0 }}>Configured Webhooks ({webhooks.length})</h3>
        </div>

        {webhooks.length === 0 ? (
          <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
            No outbound webhooks registered.
          </div>
        ) : (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>URL &amp; Description</th>
                <th style={{ padding: "0.75rem 1rem" }}>Events</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created</th>
                <th style={{ padding: "0.75rem 1rem" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {webhooks.map((w) => (
                <tr key={w.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <code style={{ fontSize: "0.85rem" }}>{w.url}</code>
                    {w.description && <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>{w.description}</div>}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <div style={{ display: "flex", gap: "0.25rem", flexWrap: "wrap" }}>
                      {w.events.map((e) => (
                        <span key={e} className="badge" style={{ fontSize: "0.7rem" }}>
                          {e}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {new Date(w.created_at).toLocaleDateString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <div style={{ display: "flex", gap: "0.5rem" }}>
                      <button
                        onClick={() => handleTest(w.id)}
                        disabled={loading}
                        className="btn btn-secondary"
                        style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem" }}
                      >
                        Test Ping
                      </button>
                      <button
                        onClick={() => handleDelete(w.id)}
                        disabled={loading}
                        className="btn btn-secondary"
                        style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem", color: "var(--bad)" }}
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
