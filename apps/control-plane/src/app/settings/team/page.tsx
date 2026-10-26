"use client";

/**
 * Team Administration & RBAC Management (SECOND.md §58).
 *
 * Governs tenant organization members, invitations, and access control
 * levels (Admin, Operator, Analyst, Viewer).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import TeamTable from "@/components/settings/team-table";
import { TeamMember } from "@/lib/api/team-api";
import { teamInvitesApi, type TeamInvite } from "@/lib/api/team-invites-api";
import { InviteActions } from "@/components/settings/InviteActions";
import { commercial } from "@/lib/commercial";

export default function TeamSettingsPage() {
  const [members, setMembers] = useState<TeamMember[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [invites, setInvites] = useState<TeamInvite[]>([]);
  const [invitesLoading, setInvitesLoading] = useState(false);
  const [invitesError, setInvitesError] = useState<string | null>(null);
  const [busyInvite, setBusyInvite] = useState<{
    id: string;
    action: "resend" | "revoke";
  } | null>(null);

  const loadMembers = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await commercial.members();
      setMembers((res.members || []).flatMap((member) => member.user ? [{
        id: member.membership_id,
        user_id: member.user.id,
        email: member.user.email,
        role: member.role,
        created_at: member.created_at,
      }] : []));
    } catch (err: unknown) {
      setMembers([]);
      setError(err instanceof Error ? err.message : "Failed to load organization members");
    } finally {
      setLoading(false);
    }
  }, []);

  const loadInvites = useCallback(async () => {
    setInvitesLoading(true);
    setInvitesError(null);
    try {
      const res = await teamInvitesApi.list();
      setInvites(res.items);
    } catch (err: unknown) {
      setInvites([]);
      setInvitesError(err instanceof Error ? err.message : "Failed to load invitations");
    } finally {
      setInvitesLoading(false);
    }
  }, []);

  const handleResend = async (id: string) => {
    setBusyInvite({ id, action: "resend" });
    try {
      await teamInvitesApi.resend(id);
      await loadInvites();
    } finally {
      setBusyInvite(null);
    }
  };

  const handleRevoke = async (id: string) => {
    setBusyInvite({ id, action: "revoke" });
    try {
      await teamInvitesApi.revoke(id);
      await loadInvites();
    } finally {
      setBusyInvite(null);
    }
  };

  useEffect(() => {
    void Promise.resolve().then(() => {
      void loadMembers();
      void loadInvites();
    });
  }, [loadMembers, loadInvites]);

  return (
    <AppShell title="Team Administration">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Team &amp; Access Governance</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Manage organization members, role-based access permissions, and operator credentials.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      {loading ? (
        <div className="card">Loading organization members...</div>
      ) : (
        <TeamTable members={members} onRefresh={() => void loadMembers()} />
      )}

      {invitesError && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginTop: "1.5rem" }}>
          {invitesError}
        </div>
      )}

      <div style={{ marginTop: "2rem" }}>
        <h3 style={{ marginTop: 0 }}>Pending Invitations ({invites.length})</h3>
        <p className="muted small" style={{ marginTop: "0.25rem" }}>
          Resend a pending invitation or revoke it. Revoking requires
          confirmation and is reflected only after a server refetch.
        </p>
        {invitesLoading ? (
          <div className="card">Loading invitations...</div>
        ) : (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Email</th>
                <th style={{ padding: "0.75rem 1rem" }}>Role</th>
                <th style={{ padding: "0.75rem 1rem" }}>Status</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created</th>
                <th style={{ padding: "0.75rem 1rem" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {invites.length === 0 ? (
                <tr>
                  <td colSpan={5} style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
                    No pending invitations.
                  </td>
                </tr>
              ) : invites.map((invite) => (
                <tr key={invite.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}><strong>{invite.email}</strong></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{invite.role}</td>
                  <td style={{ padding: "0.75rem 1rem" }}>{invite.status}</td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>
                    {new Date(invite.created_at).toLocaleDateString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <InviteActions
                      invite={invite}
                      busy={busyInvite?.id === invite.id}
                      onResend={handleResend}
                      onRevoke={handleRevoke}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </AppShell>
  );
}
