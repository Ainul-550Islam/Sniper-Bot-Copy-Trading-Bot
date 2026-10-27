"use client";

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { OrgLifecyclePanel } from "@/components/settings/OrgLifecyclePanel";
import { orgLifecycleApi, type OrganizationLifecycle } from "@/lib/api/org-lifecycle-api";
import { sessionStore } from "@/lib/auth";
import { getCapabilities, type MembershipRole } from "@/lib/permissions";
import { toDisplayError } from "@/lib/api";

function currentRole(): MembershipRole {
  const state = sessionStore.getSnapshot();
  const organization =
    state.organizations.find((item) => item.organization_id === state.selectedOrganizationId) ??
    state.organizations[0];
  const role = organization?.role.toLowerCase() ?? "";
  if (role.includes("owner")) return "owner";
  if (role.includes("admin")) return "admin";
  if (role.includes("operator") || role.includes("trader")) return "operator";
  if (role.includes("analyst") || role.includes("auditor")) return "analyst";
  return "viewer";
}

/**
 * Organization Settings (Part 04, L2).
 *
 * Shows the server-confirmed lifecycle state of the selected organization and
 * offers suspend / resume (confirmation) and close (typed slug confirmation).
 * Actions are gated to the roles the server's permission model permits; the
 * backend remains the authoritative gate.
 */
export default function OrganizationSettingsPage() {
  const [lifecycle, setLifecycle] = useState<OrganizationLifecycle | null>(null);
  const [organizationSlug, setOrganizationSlug] = useState("");
  const [organizationId, setOrganizationId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const canManage = getCapabilities({ role: currentRole() }).canManageTeam;

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const state = sessionStore.getSnapshot();
      const organization =
        state.organizations.find(
          (o) => o.organization_id === state.selectedOrganizationId,
        ) ?? state.organizations[0];
      if (!organization) throw new Error("No organization selected");
      setOrganizationId(organization.organization_id);
      setOrganizationSlug(organization.slug);
      const reported = await orgLifecycleApi.get(organization.organization_id);
      // Only a body with a real state is lifecycle truth; anything else
      // renders the panel's not-reported state instead of "undefined".
      setLifecycle(reported && typeof reported.state === "string" ? reported : null);
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Organization Settings">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Organization Lifecycle</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Server-confirmed lifecycle state of your tenant organization, with
          suspend, resume, and irreversible close controls.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {!loading && organizationId && !error && (
        <OrgLifecyclePanel
          organizationId={organizationId}
          organizationSlug={organizationSlug}
          lifecycle={lifecycle ?? null}
          canManage={canManage}
          onRefresh={() => {
            void load();
          }}
        />
      )}

      {loading && !error && <div className="card">Loading organization lifecycle…</div>}
    </AppShell>
  );
}
