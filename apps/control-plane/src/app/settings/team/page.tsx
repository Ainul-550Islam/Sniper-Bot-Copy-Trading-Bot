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
import { request } from "@/lib/api";

export default function TeamSettingsPage() {
  const [members, setMembers] = useState<TeamMember[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadMembers = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<{
        items: Array<{
          membership_id: string;
          user: { id: string; email: string; created_at: string } | null;
          role: TeamMember["role"];
          created_at: string;
        }>;
      }>("/api/saas/organizations/current/members");
      setMembers((res.items || []).flatMap((member) => member.user ? [{
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

  useEffect(() => {
    void Promise.resolve().then(() => loadMembers());
  }, [loadMembers]);

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
    </AppShell>
  );
}
