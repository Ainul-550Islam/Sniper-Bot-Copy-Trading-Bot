"use client";

import { useCallback, useEffect, useState } from "react";
import type { ReconcileResponse } from "@/lib/api/billing-status-api";
import { billingStatusApi } from "@/lib/api/billing-status-api";
import { toDisplayError } from "@/lib/api";

interface ReconcilePanelProps {
  organizationId: string;
  canManage: boolean;
}

/**
 * Reconciliation panel: trigger `POST /api/saas/billing/reconcile` (roles the
 * server permits only) and render the server-confirmed result from
 * `GET /api/saas/billing/reconcile/{id}`. No optimistic status; the result
 * appears only after a refetch.
 */
export function ReconcilePanel({ organizationId, canManage }: ReconcilePanelProps) {
  const [result, setResult] = useState<ReconcileResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [resultLoading, setResultLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const loadResult = useCallback(async () => {
    setResultLoading(true);
    setError(null);
    try {
      const reported = await billingStatusApi.reconcileResult(organizationId);
      // Only show a decision the server actually reported; an empty body
      // is treated as "no reconciliation on record", never as a fake result.
      setResult(reported && typeof reported.action === "string" && typeof reported.decided_at === "string" ? reported : null);
    } catch (err: unknown) {
      // A 404 means no reconciliation has run yet — that is an honest empty
      // state, not a failure.
      if (err instanceof Error && /404/.test(err.message)) {
        setResult(null);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setResultLoading(false);
    }
  }, [organizationId]);

  useEffect(() => {
    void Promise.resolve().then(() => loadResult());
  }, [loadResult]);

  const handleReconcile = async () => {
    if (!window.confirm("Reconcile subscription and entitlement state with the billing provider? This re-reads provider records for this organization.")) return;
    setLoading(true);
    setError(null);
    setMessage(null);
    try {
      await billingStatusApi.reconcile();
      setMessage("Reconciliation accepted — result below is refreshed from the server.");
      await loadResult();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <section className="card" aria-label="Billing reconciliation">
      <h2>Billing Reconciliation</h2>
      <p className="muted small">
        Reconciles the local subscription state against the billing provider&apos;s
        records. The action below is allowed only for roles the server permits,
        and the displayed result always comes from a server refetch.
      </p>

      <div style={{ marginTop: "1rem", display: "flex", gap: "0.75rem", flexWrap: "wrap" }}>
        {!canManage && (
          <p className="muted">Your current role can view reconciliation results but cannot trigger reconciliation.</p>
        )}
        {canManage && (
          <button
            type="button"
            disabled={loading || !canManage}
            onClick={handleReconcile}
            className="btn btn-primary"
          >
            {loading ? "Reconciling…" : "Reconcile with Provider"}
          </button>
        )}
      </div>

      {message && <div style={{ marginTop: "0.75rem", color: "var(--ok)", fontSize: "0.85rem" }}>{message}</div>}
      {error && (
        <div style={{ marginTop: "0.75rem", color: "var(--bad)", background: "var(--bad-glow)", padding: "0.75rem", borderRadius: "4px" }}>
          {error}
        </div>
      )}

      <div style={{ marginTop: "1rem" }}>
        <h3 style={{ marginTop: 0 }}>Last Reconciliation Result</h3>
        {resultLoading && <p>Loading reconciliation result…</p>}
        {result === null && resultLoading === false && (
          <p className="muted">No reconciliation has been recorded for this organization yet.</p>
        )}
        {result && resultLoading === false && (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "0.5rem" }}>
            <tbody>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Action</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{result.action}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Reason</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{result.reason}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Decided At</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{new Date(result.decided_at).toLocaleString()}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Idempotent</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{String(result.idempotent)}</td>
              </tr>
              {result.provider_event_kind && (
                <tr style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}><strong>Provider Event</strong></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{result.provider_event_kind}</td>
                </tr>
              )}
              {result.internal_subscription_status && (
                <tr style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}><strong>Internal Subscription Status</strong></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{result.internal_subscription_status}</td>
                </tr>
              )}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}
