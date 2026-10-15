"use client";

import { useCallback, useEffect, useState } from "react";

import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import {
  listWebhookDeliveries,
  redeliverWebhookDelivery,
  type WebhookDelivery,
} from "@/lib/api/webhook-api";

interface WebhookDeliveriesProps {
  endpointId: string;
}

const REDELIVERABLE = new Set(["dead", "failed"]);

/**
 * Delivery history for one webhook endpoint (File 3 §7). Rows come straight
 * from the server; nothing here invents a status. "Redeliver" is offered only
 * for rows the server reports as dead or failed.
 */
export function WebhookDeliveries({ endpointId }: WebhookDeliveriesProps) {
  const [items, setItems] = useState<WebhookDelivery[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      setItems(await listWebhookDeliveries(endpointId));
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Deliveries could not be loaded.");
    }
  }, [endpointId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  async function redeliver(delivery: WebhookDelivery) {
    setBusyId(delivery.id);
    setNotice(null);
    try {
      await redeliverWebhookDelivery(endpointId, delivery.id);
      setNotice(`Redelivery queued for event ${delivery.event_id}.`);
      await load();
    } catch (err: unknown) {
      setNotice(err instanceof Error ? err.message : "Redelivery failed.");
    } finally {
      setBusyId(null);
    }
  }

  if (error) {
    return <ErrorState title="Webhook deliveries unavailable" error={error} onRetry={() => void load()} />;
  }
  if (items === null) {
    return <div style={{ color: "var(--muted)", padding: "0.5rem" }}>Loading deliveries…</div>;
  }
  if (items.length === 0) {
    return (
      <EmptyState
        title="No deliveries yet"
        description="Deliveries appear here after an event is sent to this endpoint."
      />
    );
  }

  return (
    <div>
      {notice ? (
        <div role="status" style={{ marginBottom: "0.5rem", color: "var(--muted)", fontSize: "0.85rem" }}>
          {notice}
        </div>
      ) : null}
      <div style={{ overflowX: "auto" }}>
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: "0.85rem" }}>
          <thead>
            <tr style={{ textAlign: "left", color: "var(--muted)" }}>
              <th>Event</th>
              <th>Status</th>
              <th>HTTP</th>
              <th>Attempts</th>
              <th>Next retry</th>
              <th>Last error</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {items.map((d) => (
              <tr key={d.id} style={{ borderTop: "1px solid var(--line)" }}>
                <td>{d.event_type}</td>
                <td>{d.status}</td>
                <td>{d.response_code ?? "—"}</td>
                <td>{d.attempts}</td>
                <td>{d.next_retry_at ? new Date(d.next_retry_at).toLocaleString() : "—"}</td>
                <td style={{ maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis" }}>
                  {d.last_error ?? "—"}
                </td>
                <td>
                  {REDELIVERABLE.has(d.status) ? (
                    <button
                      type="button"
                      className="btn btn-secondary"
                      disabled={busyId === d.id}
                      onClick={() => void redeliver(d)}
                    >
                      {busyId === d.id ? "Queuing…" : "Redeliver"}
                    </button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export default WebhookDeliveries;
