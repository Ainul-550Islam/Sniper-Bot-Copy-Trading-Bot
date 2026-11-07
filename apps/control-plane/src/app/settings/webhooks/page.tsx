"use client";

/**
 * Outbound webhook management.
 *
 * The page renders only the server's tenant-scoped records. It never replaces
 * an unavailable API with a sample endpoint, because doing so can make an
 * operator believe that notifications are configured when they are not.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import WebhookForm from "@/components/settings/webhook-form";
import { WebhookEndpoint, listWebhooks } from "@/lib/api/webhook-api";
import { toDisplayError } from "@/lib/api";
import { sessionStore } from "@/lib/auth";

export default function WebhooksPage() {
  const [webhooks, setWebhooks] = useState<WebhookEndpoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setWebhooks([]);
    setLoading(true);
    setError(null);
  }), []);

  const loadWebhooks = useCallback(async () => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setWebhooks([]);
    setLoading(true);
    setError(null);
    if (!selectedOrganizationId) {
      setLoading(false);
      return;
    }
    try {
      const data = await listWebhooks();
      if (requestId !== requestVersion.current) return;
      if (data.some((webhook) => webhook.organization_id !== selectedOrganizationId)) {
        throw new Error("Webhook endpoint response does not match the selected organization");
      }
      setWebhooks(data);
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setWebhooks([]);
        setError(toDisplayError(err));
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void loadWebhooks();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [loadWebhooks, sessionVersion]);

  return (
    <AppShell title="Webhooks">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Outbound Webhooks &amp; Event Relayers</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Subscribe external systems to execution events, risk alerts, and lifecycle status changes.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          <div>{error}</div>
          <button type="button" onClick={() => void loadWebhooks()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
            Retry
          </button>
        </div>
      )}

      {loading ? (
        <div className="card">Loading webhook endpoints...</div>
      ) : !selectedOrganizationId ? (
        <div className="card">Select an organization to manage webhook endpoints.</div>
      ) : (
        <WebhookForm
          key={selectedOrganizationId}
          webhooks={webhooks}
          onRefresh={() => void loadWebhooks()}
        />
      )}
    </AppShell>
  );
}
