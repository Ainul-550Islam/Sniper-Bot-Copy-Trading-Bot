"use client";

import { useEffect, useState } from "react";
import { commercial, toDisplayError, LifecycleStatus } from "@/lib/commercial";
import { request } from "@/lib/api";

export default function DataLifecyclePage() {
  const [status, setStatus] = useState<LifecycleStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  async function load() {
    setError(null);
    try {
      // We need organizationId — fetch from current user membership via /api/saas/users/me
      const me = await request<{ organizations?: Array<{ organization_id: string }> }>("/api/saas/users/me").catch(() => null);
      const orgId = (me as any)?.organizations?.[0]?.organization_id ?? (me as any)?.user?.id ?? null;
      // Fallback: try billing status to get org
      let targetOrg = orgId;
      if (!targetOrg) {
        const b = await commercial.billingStatus().catch(() => null);
        targetOrg = b?.organization_id ?? null;
      }
      if (!targetOrg) throw new Error("No organization found for lifecycle status");
      const s = await commercial.lifecycleStatus(targetOrg);
      setStatus(s);
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  async function requestClose() {
    if (confirm !== "CLOSE") { setError("Please type CLOSE to confirm"); return; }
    setBusy(true); setError(null); setMessage(null);
    try {
      const orgId = status?.organization_id;
      if (!orgId) throw new Error("No organization");
      await request(`/api/saas/organizations/${encodeURIComponent(orgId)}/suspension`, {
        method: "POST",
        body: { reason: "customer requested closure" },
      });
      setMessage("Closure requested — trading is now disabled. Credentials will be revoked, sessions invalidated, and custody drained. Retention will schedule purge eligibility. This action is irreversible.");
      await load();
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setBusy(false);
    }
  }

  if (loading) return <main className="card"><p>Loading lifecycle status…</p></main>;

  return (
    <main id="main" className="stack">
      <h1>Data &amp; Lifecycle</h1>
      {error && <p role="alert" className="error">{error}</p>}
      {message && <p className="success">{message}</p>}
      {status ? (
        <section className="card">
          <h2>Tenant Status: {status.organization_status}</h2>
          <dl>
            <dt>Phase</dt><dd>{status.phase}</dd>
            <dt>Organization</dt><dd>{status.organization_id}</dd>
            <dt>Custody revoked</dt><dd>{status.custody_revoked ? "yes" : "no"}</dd>
            <dt>Sessions invalidated</dt><dd>{status.sessions_invalidated ? "yes" : "no"}</dd>
            <dt>Retention scheduled</dt><dd>{status.retention_scheduled ? "yes" : "no"}</dd>
            <dt>Purge eligible</dt><dd>{status.purge_eligible_at ?? "not yet"}</dd>
          </dl>
          <p className="muted small">Suspended tenants cannot trade or manage resources but can read and reduce. Closed tenants are denied everything.</p>
        </section>
      ) : <p className="muted">No lifecycle data.</p>}

      <section className="card">
        <h2>Request Closure / Deprovision</h2>
        <p className="muted">This will irreversibly close the tenant, disable trading, revoke credentials and API keys, invalidate sessions, disconnect websockets, and schedule retention. Financial and audit records are preserved.</p>
        <label>
          Type <code>CLOSE</code> to confirm
          <input value={confirm} onChange={(e) => setConfirm(e.target.value)} placeholder="CLOSE" />
        </label>
        <button className="primary" disabled={busy || confirm !== "CLOSE"} onClick={requestClose}>
          {busy ? "Working…" : "Request closure"}
        </button>
      </section>
    </main>
  );
}
