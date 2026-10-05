"use client";

import { useState } from "react";
import { TeamMember, TeamRole, inviteMember, removeMember } from "@/lib/api/team-api";

interface TeamTableProps {
  members: TeamMember[];
  onRefresh: () => void;
}

export default function TeamTable({ members, onRefresh }: TeamTableProps) {
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<TeamRole>("trader");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  const handleInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    setSuccess(null);

    try {
      await inviteMember({ email, role });
      setSuccess(`Invitation successfully sent to ${email}`);
      setEmail("");
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to invite team member");
    } finally {
      setLoading(false);
    }
  };

  const handleRemove = async (member: TeamMember) => {
    if (!confirm(`Revoke access for ${member.email}?`)) return;
    setLoading(true);
    try {
      await removeMember(member.id);
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to remove member");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div>
      <div className="card" style={{ marginBottom: "1.5rem" }}>
        <h3 style={{ marginTop: 0 }}>Invite Team Member</h3>
        {error && (
          <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
            {error}
          </div>
        )}
        {success && (
          <div style={{ color: "var(--ok)", background: "var(--ok-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
            {success}
          </div>
        )}

        <form onSubmit={handleInvite} style={{ display: "flex", gap: "1rem", alignItems: "flex-end", flexWrap: "wrap" }}>
          <div style={{ flex: 2, minWidth: "240px" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Work Email Address
            </label>
            <input
              type="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="operator@your-domain.example"
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div style={{ flex: 1, minWidth: "160px" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Role / Permissions
            </label>
            <select
              value={role}
              onChange={(e) => setRole(e.target.value as TeamRole)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="org_admin">Organization Admin</option>
              <option value="trader">Trader (Trading & Bots)</option>
              <option value="auditor">Auditor (Read & Audit)</option>
              <option value="viewer">Viewer (Read Only)</option>
            </select>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary" style={{ height: "38px" }}>
            {loading ? "Sending..." : "Send Invitation"}
          </button>
        </form>
      </div>

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <h3 style={{ margin: 0 }}>Active Team Members ({members.length})</h3>
        </div>

        <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Member Email</th>
              <th style={{ padding: "0.75rem 1rem" }}>Role</th>
              <th style={{ padding: "0.75rem 1rem" }}>Joined Date</th>
              <th style={{ padding: "0.75rem 1rem" }}>Actions</th>
            </tr>
          </thead>
          <tbody>
            {members.length === 0 ? (
              <tr>
                <td colSpan={4} style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
                  No organization members were returned.
                </td>
              </tr>
            ) : members.map((m) => (
              <tr key={m.id} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <strong>{m.email}</strong>
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
                    {m.role.toUpperCase()}
                  </span>
                </td>
                <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>
                  {new Date(m.created_at).toLocaleDateString()}
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  {m.role !== "org_owner" && (
                    <button
                      onClick={() => handleRemove(m)}
                      className="btn btn-secondary"
                      style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem", color: "var(--bad)" }}
                    >
                      Revoke
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
