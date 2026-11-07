"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { OrgLifecyclePanel } from "@/components/settings/OrgLifecyclePanel";
import { orgLifecycleApi, type OrganizationLifecycle } from "@/lib/api/org-lifecycle-api";
import { sessionStore } from "@/lib/auth";
import { getCapabilities, type MembershipRole } from "@/lib/permissions";
import { toDisplayError } from "@/lib/api";

function currentRole(organizationId: string | null): MembershipRole {
  const state = sessionStore.getSnapshot();
  const organization = organizationId
    ? state.organizations.find((item) => item.organization_id === organizationId)
    : state.organizations.length === 1 ? state.organizations[0] : undefined;
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
  const [selectedOrganizationId, setSelectedOrganizationId] = useState(
    () => sessionStore.getSnapshot().selectedOrganizationId,
  );
  const [sessionVersion, setSessionVersion] = useState(0);
  const requestVersion = useRef(0);

  const canManage = getCapabilities({ role: currentRole(selectedOrganizationId) }).canManageTeam;

  useEffect(() => sessionStore.subscribe(() => {
    setSelectedOrganizationId(sessionStore.getSnapshot().selectedOrganizationId);
    setSessionVersion((version) => version + 1);
    setLifecycle(null);
    setOrganizationId(null);
    setOrganizationSlug("");
    setLoading(true);
  }), []);

  const load = useCallback(async () => {
    const version = ++requestVersion.current;
    setLoading(true);
    setError(null);
    setLifecycle(null);
    setOrganizationId(null);
    setOrganizationSlug("");
    try {
      const state = sessionStore.getSnapshot();
      const organization = selectedOrganizationId
        ? state.organizations.find((item) => item.organization_id === selectedOrganizationId) ?? null
        : state.organizations.length === 1 ? state.organizations[0] ?? null : null;
      if (!organization) throw new Error("No selected organization is available");
      const reported = await orgLifecycleApi.get(organization.organization_id);
      if (version !== requestVersion.current) return;
      // Only a body with a real state is lifecycle truth; anything else
      // renders the panel's not-reported state instead of "undefined".
      setOrganizationId(organization.organization_id);
      setOrganizationSlug(organization.slug);
      setLifecycle(reported && typeof reported.state === "string" ? reported : null);
    } catch (err: unknown) {
      if (version === requestVersion.current) setError(toDisplayError(err));
    } finally {
      if (version === requestVersion.current) setLoading(false);
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

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
