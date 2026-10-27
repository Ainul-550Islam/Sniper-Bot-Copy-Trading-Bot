"use client";

import { useState } from "react";
import type { TeamInvite } from "@/lib/api/team-invites-api";
import { toDisplayError } from "@/lib/api";

interface InviteActionsProps {
  invite: TeamInvite;
  busy: boolean;
  onResend: (id: string) => void;
  onRevoke: (id: string) => void;
}

/**
 * Per-invite action cell: Resend and Revoke.
 *
 * Rendering this component performs no request. Resend posts immediately;
 * Revoke is destructive and is gated behind `window.confirm`, so the DELETE
 * is never sent when the operator cancels. The parent owns the API call and
 * the subsequent refetch, so status always comes from server truth.
 */
export function InviteActions({
  invite,
  busy,
  onResend,
  onRevoke,
}: InviteActionsProps) {
  const [error, setError] = useState<string | null>(null);

  const handleResend = async () => {
    setError(null);
    try {
      await onResend(invite.id);
    } catch (err: unknown) {
      setError(toDisplayError(err));
    }
  };

  const handleRevoke = async () => {
    if (!window.confirm(`Revoke invitation for ${invite.email}?`)) return;
    setError(null);
    try {
      await onRevoke(invite.id);
    } catch (err: unknown) {
      setError(toDisplayError(err));
    }
  };

  return (
    <div>
      <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap", alignItems: "center" }}>
        <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
          {invite.status}
        </span>
        <button
          type="button"
          disabled={busy}
          onClick={handleResend}
          className="btn btn-secondary"
          style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem" }}
        >
          {busy ? "Sending…" : "Resend"}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={handleRevoke}
          className="btn btn-secondary danger"
          style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem", color: "var(--bad)" }}
        >
          Revoke
        </button>
      </div>
      {error && (
        <div style={{ marginTop: "0.35rem", color: "var(--bad)", fontSize: "0.8rem" }}>
          {error}
        </div>
      )}
    </div>
  );
}

export default InviteActions;
