"use client";

import React, { useEffect, useState } from "react";
import Link from "next/link";
import { sessionStore } from "@/lib/auth";
import { getPermissions, type MembershipRole } from "@/lib/permissions";

export interface OperatorGuardProps {
  children: React.ReactNode;
  fallback?: React.ReactNode;
}

/**
 * Client-side guard for operator surfaces.
 *
 * Verifies that the active user or selected organization membership has
 * operator read permissions before rendering children. If unauthorized,
 * renders an explicit 403 / Access Denied view and blocks rendering of
 * children so no operator API fetch is ever triggered.
 */
export function OperatorGuard({ children, fallback }: OperatorGuardProps) {
  const [, forceRender] = useState(0);

  useEffect(() => {
    return sessionStore.subscribe(() => forceRender((n) => n + 1));
  }, []);

  const state = sessionStore.getSnapshot();
  const user = state.user;
  const currentOrg =
    state.organizations.find((o) => o.organization_id === state.selectedOrganizationId) ??
    state.organizations[0];

  const permissions = getPermissions(
    {
      role: (currentOrg?.role?.toLowerCase() as MembershipRole) ?? "viewer",
    },
    user,
  );

  if (!permissions.operator.read) {
    if (fallback) {
      return <>{fallback}</>;
    }

    return (
      <div className="card" style={{ maxWidth: "600px", margin: "2rem auto", textAlign: "center" }}>
        <h2 style={{ color: "var(--bad)" }}>Operator Access Required</h2>
        <p className="muted" style={{ margin: "1rem 0" }}>
          You do not have permission to view or control the system operator surface.
          Platform administrator or organization operator permissions are required.
        </p>
        <div style={{ marginTop: "1.5rem" }}>
          <Link href="/">
            <button type="button" className="primary">
              Return to Dashboard
            </button>
          </Link>
        </div>
      </div>
    );
  }

  return <>{children}</>;
}

export default OperatorGuard;
