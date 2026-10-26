"use client";

import { useEffect, useState, useCallback } from "react";
import { AppShell } from "@/components/AppShell";
import { commercial, toDisplayError, LifecycleStatus } from "@/lib/commercial";
import { request, exportsApi, ApiError, type ExportEnvelope } from "@/lib/api";
import { LifecycleActions, type LifecycleAction } from "@/components/lifecycle/LifecycleActions";
import { lifecycleActionsApi } from "@/lib/api/lifecycle-actions-api";
import { getCapabilities, type MembershipRole } from "@/lib/permissions";

function membershipRole(role: string | undefined): MembershipRole {
  const normalized = role?.toLowerCase() ?? "";
  if (normalized.includes("owner")) return "owner";
  if (normalized.includes("admin")) return "admin";
  if (normalized.includes("operator") || normalized.includes("trader")) return "operator";
  if (normalized.includes("analyst") || normalized.includes("auditor")) return "analyst";
  return "viewer";
}

export default function DataLifecyclePage() {
  const [status, setStatus] = useState<LifecycleStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [exportData, setExportData] = useState<ExportEnvelope | null>(null);
  const [exportKind, setExportKind] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<LifecycleAction | null>(null);
  const [canManageLifecycle, setCanManageLifecycle] = useState(false);

  const loadData = useCallback(async () => {
    setError(null);
    try {
      const me = await request<{ organizations?: Array<{ organization_id: string; role: string }> }>("/api/saas/users/me");
      const organization = me.organizations?.[0] ?? null;
      const targetOrg = organization?.organization_id ?? null;
      setCanManageLifecycle(
        getCapabilities({ role: membershipRole(organization?.role) }).canManageTeam,
      );
      if (!targetOrg) throw new Error("No organization found for lifecycle status");
      const s = await commercial.lifecycleStatus(targetOrg);
      setStatus(s);
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  async function runLifecycleAction(action: LifecycleAction) {
    const orgId = status?.organization_id;
    if (!orgId) {
      setError("No active organization identified");
      return;
    }
    setBusyAction(action);
    setError(null);
    setMessage(null);
    try {
      if (action === "disconnect") await lifecycleActionsApi.disconnect(orgId);
      if (action === "revoke-credentials") await lifecycleActionsApi.revokeCredentials(orgId);
      if (action === "revoke-sessions") await lifecycleActionsApi.revokeSessions(orgId);
      if (action === "schedule-retention") {
        await lifecycleActionsApi.scheduleRetention(orgId, { confirm: true });
      }
      setMessage("Lifecycle action accepted by the server; status refreshed below.");
      await loadData();
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleExport(kind: string) {
    setExportKind(kind);
    try {
      const res = await exportsApi.get(kind);
      setExportData(res);
    } catch (e) {
      setError(e instanceof ApiError ? `${e.kind}: ${e.reason}` : "Export failed");
    }
  }

  async function requestClose() {
    if (confirm !== "CLOSE") {
      setError("Please type CLOSE to confirm account deprovisioning");
      return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const orgId = status?.organization_id;
      if (!orgId) throw new Error("No active organization identified");
      await request(`/api/saas/organizations/${encodeURIComponent(orgId)}/suspension`, {
        method: "POST",
        body: { reason: "Customer requested organization deprovisioning and closure" },
      });
      setMessage("Closure requested — trading is now disabled. Credentials will be revoked and retention policy scheduled.");
      await loadData();
    } catch (e) {
      setError(toDisplayError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Data Lifecycle &amp; Privacy Compliance</h1>
            <p className="muted">
              Deterministic audit exports, legal data retention policies, and formal tenant deprovisioning controls.
            </p>
          </div>
        </div>

        {error && <div className="notice danger">{error}</div>}
        {message && <div className="notice success">{message}</div>}

        {loading ? (
          <p className="muted">Loading lifecycle state…</p>
        ) : (
          <>
            {/* Status Card */}
            {status && (
              <section className="card">
                <h2>Tenant Lifecycle State: {status.organization_status.toUpperCase()}</h2>
                <div className="grid-3" style={{ marginTop: "1rem" }}>
                  <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                    <span className="stat-card__title">Lifecycle Phase</span>
                    <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{status.phase.toUpperCase()}</span>
                  </div>
                  <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                    <span className="stat-card__title">Retention Scheduled</span>
                    <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{status.retention_scheduled ? "YES" : "NO"}</span>
                  </div>
                  <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                    <span className="stat-card__title">Purge Eligible At</span>
                    <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{status.purge_eligible_at ?? "Not Scheduled"}</span>
                  </div>
                </div>
              </section>
            )}

            {status && (
              <LifecycleActions
                organizationId={status.organization_id}
                canManage={canManageLifecycle}
                busyAction={busyAction}
                onAction={(action) => void runLifecycleAction(action)}
              />
            )}

            {/* Deterministic Data Exports */}
            <section className="card">
              <h2>Deterministic Data Exports and Audit Records</h2>
              <p className="muted small">
                Download cryptographically verifiable exports of your organization&apos;s complete ledger, orders, and audit trail.
              </p>
              <div className="row" style={{ marginTop: "1rem" }}>
                {["audit", "subscription", "usage", "wallets", "api_keys", "profile"].map((k) => (
                  <button key={k} type="button" onClick={() => void handleExport(k)}>
                    Export {k.toUpperCase()}
                  </button>
                ))}
              </div>

              {exportData && (
                <div style={{ marginTop: "1rem" }}>
                  <h3>Export: {exportKind}</h3>
                  <pre className="json">{JSON.stringify(exportData, null, 2)}</pre>
                </div>
              )}
            </section>

            {/* Tenant Deprovisioning */}
            <section className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
              <h2>Request Organization Closure &amp; Data Purge</h2>
              <p className="muted small">
                Closing an organization permanently disables automated trading, revokes API keys, disconnects WebSockets,
                and schedules data purge according to statutory financial retention requirements.
              </p>
              <div className="form" style={{ marginTop: "1rem", maxWidth: "480px" }}>
                <div className="form-group">
                  <label htmlFor="confirmClose">Type <code>CLOSE</code> to confirm</label>
                  <input
                    id="confirmClose"
                    value={confirm}
                    onChange={(e) => setConfirm(e.target.value)}
                    placeholder="CLOSE"
                  />
                </div>
                <button
                  className="danger"
                  disabled={busy || confirm !== "CLOSE"}
                  onClick={() => void requestClose()}
                >
                  {busy ? "Processing…" : "Request Irreversible Deprovisioning"}
                </button>
              </div>
            </section>
          </>
        )}
      </div>
    </AppShell>
  );
}
