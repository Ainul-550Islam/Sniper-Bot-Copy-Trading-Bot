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
      const res = await request<{ items: TeamMember[] }>("/api/saas/organizations/current/members");
      setMembers(res.items || []);
    } catch {
      // Fallback sample members for demo / local development
      setMembers([
        {
          id: "mem-01",
          user_id: "usr-01",
          email: "lead.trader@acme-quant.com",
          role: "owner",
          created_at: new Date(Date.now() - 30 * 86400000).toISOString(),
        },
        {
          id: "mem-02",
          user_id: "usr-02",
          email: "risk.officer@acme-quant.com",
          role: "admin",
          created_at: new Date(Date.now() - 15 * 86400000).toISOString(),
        },
      ]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadMembers();
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
