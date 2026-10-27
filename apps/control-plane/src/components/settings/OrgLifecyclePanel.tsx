"use client";

import { useState } from "react";
import type { OrganizationLifecycle } from "@/lib/api/org-lifecycle-api";
import { orgLifecycleApi } from "@/lib/api/org-lifecycle-api";
import { toDisplayError } from "@/lib/api";

interface OrgLifecyclePanelProps {
  organizationId: string;
  organizationSlug: string;
  lifecycle: OrganizationLifecycle | null;
  canManage: boolean;
  onRefresh: () => void;
}

/**
 * Organization lifecycle panel: current server-confirmed state, suspend /
 * resume with confirmation, and close through a typed confirmation where the
 * operator types the organization slug. Rendering performs no request; the
 * DELETE/POST is sent only after confirmation, and the displayed state is
 * refetched from the server afterward.
 */
export function OrgLifecyclePanel({
  organizationId,
  organizationSlug,
  lifecycle,
  canManage,
  onRefresh,
}: OrgLifecyclePanelProps) {
  const [error, setError] = useState<string | null>(null);
  const [closeReason, setCloseReason] = useState("");
  const [busy, setBusy] = useState(false);

  const handleSuspend = async () => {
    if (
      !window.confirm(
        `Suspend organization "${organizationSlug}"? Trading and API access will be blocked.`,
      )
    )
      return;
    setBusy(true);
    setError(null);
    try {
      await orgLifecycleApi.suspend(organizationId);
      await onRefresh();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setBusy(false);
    }
  };

  const handleResume = async () => {
    if (!window.confirm(`Resume organization "${organizationSlug}"?`)) return;
    setBusy(true);
    setError(null);
    try {
      await orgLifecycleApi.resume(organizationId);
      await onRefresh();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setBusy(false);
    }
  };

  const handleClose = async () => {
    setError(null);
    setBusy(true);
    try {
      await orgLifecycleApi.close(organizationId, closeReason);
      await onRefresh();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setBusy(false);
    }
  };

  const canAct = canManage && !busy;

  if (!lifecycle) {
    return (
      <section className="card" aria-label="Organization lifecycle">
        <h2>Organization Lifecycle</h2>
        <p className="muted">Loading lifecycle state from the server…</p>
      </section>
    );
  }

  const suspendedOrClosed = lifecycle.state === "suspended" || lifecycle.state === "closed";

  return (
    <section className="card" aria-label="Organization lifecycle">
      <h2>Organization Lifecycle</h2>
      <p className="muted small">
        Lifecycle state below is the server-confirmed truth for organization
        <code>{organizationId}</code> (slug <code>{organizationSlug}</code>).
      </p>

      <div className="grid-3" style={{ marginTop: "1rem" }}>
        <div className="stat-card" style={{ background: "var(--panel-2)" }}>
          <span className="stat-card__title">Lifecycle Status</span>
          <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>
            {lifecycle.lifecycle_status}
          </span>
          <span className="muted small" style={{ marginTop: "0.3rem", display: "block" }}>
            State: {lifecycle.state}
          </span>
        </div>
        <div className="stat-card" style={{ background: "var(--panel-2)" }}>
          <span className="stat-card__title">Suspended</span>
          <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>
            {lifecycle.state === "suspended" ? "Yes" : "No"}
          </span>
          <span className="muted small" style={{ marginTop: "0.3rem", display: "block" }}>
            {lifecycle.suspended_at ? `Since ${new Date(lifecycle.suspended_at).toLocaleString()}` : "—"}
          </span>
        </div>
        <div className="stat-card" style={{ background: "var(--panel-2)" }}>
          <span className="stat-card__title">Closed</span>
          <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>
            {lifecycle.state === "closed" ? "Yes" : "No"}
          </span>
          <span className="muted small" style={{ marginTop: "0.3rem", display: "block" }}>
            {lifecycle.closed_at ? `Closed ${new Date(lifecycle.closed_at).toLocaleString()}` : "—"}
          </span>
        </div>
      </div>

      {lifecycle.reason && (
        <p className="muted small" style={{ marginTop: "0.5rem" }}>
          Reason: {lifecycle.reason}
        </p>
      )}

      <div style={{ marginTop: "1rem", display: "flex", gap: "0.75rem", flexWrap: "wrap" }}>
        {!canManage && (
          <p className="muted">
            Your current role can view lifecycle state but cannot suspend, resume,
            or close the organization.
          </p>
        )}
        {canManage && (
          <>
            <button
              type="button"
              disabled={busy || suspendedOrClosed || !canManage}
              onClick={handleSuspend}
              className="btn btn-secondary"
            >
              {busy ? "Processing…" : suspendedOrClosed ? "Not actionable" : "Suspend organization"}
            </button>
            <button
              type="button"
              disabled={busy || lifecycle.state !== "suspended" || !canManage}
              onClick={handleResume}
              className="btn btn-secondary"
            >
              {busy ? "Processing…" : lifecycle.state !== "suspended" ? "Not suspended" : "Resume organization"}
            </button>
          </>
        )}
      </div>

      <div style={{ marginTop: "1rem", borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
        <h3>Close Organization</h3>
        <p className="muted small">
          Closing is irreversible: trading stops, API keys are revoked, and data
          purge is scheduled according to retention policy. Type the organization
          slug below to confirm.
        </p>
        {!canManage && (
          <p className="muted">Your current role cannot close this organization.</p>
        )}
        <div className="form-group" style={{ marginTop: "0.5rem", maxWidth: "420px" }}>
          <label htmlFor="org-close-slug">
            Type <code>{organizationSlug}</code> to confirm
          </label>
          <input
            id="org-close-slug"
            value={closeReason}
            onChange={(event) => setCloseReason(event.target.value)}
            disabled={!canManage || busy}
            autoComplete="off"
            placeholder={organizationSlug}
            className="input"
            style={{ width: "100%" }}
          />
        </div>
        <div className="row" style={{ marginTop: "0.5rem" }}>
          <button
            type="button"
            className="danger"
            disabled={
              !canAct ||
              busy ||
              lifecycle.state === "closed" ||
              closeReason !== organizationSlug
            }
            onClick={handleClose}
          >
            {busy ? "Closing…" : "Confirm close organization"}
          </button>
          {lifecycle.state === "closed" && (
            <span className="badge" style={{ marginLeft: "0.5rem", background: "var(--bad-glow)" }}>
              Closed
            </span>
          )}
        </div>
        {error && (
          <div style={{ marginTop: "0.5rem", color: "var(--bad)", fontSize: "0.85rem" }}>
            {error}
          </div>
        )}
      </div>

      {lifecycle.state === "closed" && (
        <div style={{ marginTop: "0.75rem", color: "var(--bad)", fontSize: "0.85rem" }}>
          Organization <code>{organizationSlug}</code> is closed. Reopening is not supported through this console.
        </div>
      )}
    </section>
  );
}
