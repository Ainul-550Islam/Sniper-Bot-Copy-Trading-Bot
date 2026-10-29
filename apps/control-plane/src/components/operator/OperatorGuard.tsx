"use client";

import React, { useEffect, useState } from "react";
import Link from "next/link";
import { sessionStore } from "@/lib/auth";
import { getOperatorPermissions } from "@/lib/permissions";
import {
  clearOperatorKey,
  operatorSessionStore,
  setOperatorKey,
} from "@/lib/operator-session";
import { OperatorKeyPrompt } from "@/components/operator/OperatorKeyPrompt";

export interface OperatorGuardProps {
  children: React.ReactNode;
  fallback?: React.ReactNode;
}

/**
 * Client-side guard for operator surfaces. Three states, in order:
 *
 * 1. NOT A PLATFORM ADMIN → rendered access-denied, and `children` are never
 *    mounted, so no operator request is ever attempted. This is the state that
 *    used to leak: tenant owner/admin/operator memberships were mapped onto the
 *    operator console even though the backend authorizes that plane from the
 *    operator KEY, never from a tenant role.
 * 2. PLATFORM ADMIN WITHOUT A KEY → the unlock prompt. Still no operator
 *    request: `operator-api` throws `OperatorKeyRequiredError` before a fetch
 *    is built, and the guard never mounts the children that would make one.
 * 3. PLATFORM ADMIN WITH A KEY → `children`, plus a Lock control that drops
 *    the key from memory.
 *
 * None of this is the authorization boundary: the backend re-proves the key's
 * role on every operator route, and a 401 clears the key (see
 * `lib/api/operator-api.ts`), which drops this component back to state 2 with
 * the server's reason.
 */
export function OperatorGuard({ children, fallback }: OperatorGuardProps) {
  const [, forceRender] = useState(0);

  useEffect(() => sessionStore.subscribe(() => forceRender((n) => n + 1)), []);
  useEffect(
    () => operatorSessionStore.subscribe(() => forceRender((n) => n + 1)),
    [],
  );

  const user = sessionStore.getSnapshot().user;
  const operator = operatorSessionStore.getSnapshot();

  // Visibility only. The role argument is gone on purpose: a tenant membership
  // role carries no operator authority.
  const permissions = getOperatorPermissions(user);

  if (!permissions.read) {
    if (fallback) {
      return <>{fallback}</>;
    }

    return (
      <div className="card" style={{ maxWidth: "600px", margin: "2rem auto", textAlign: "center" }}>
        <h2 style={{ color: "var(--bad)" }}>Operator Access Required</h2>
        <p className="muted" style={{ margin: "1rem 0" }}>
          You do not have permission to view or control the system operator surface.
          Platform administrator permissions are required, and operator authority itself
          is granted by an operator API key that the backend verifies on every request.
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

  if (operator.key === null) {
    return (
      <OperatorKeyPrompt
        onUnlock={setOperatorKey}
        error={operator.lastRefusal}
      />
    );
  }

  return (
    <div className="stack">
      <div className="row-between" style={{ alignItems: "center", gap: "0.5rem" }}>
        <span className="badge">
          Operator console unlocked · key in memory only · locks itself after 15
          minutes idle
        </span>
        <button type="button" onClick={() => clearOperatorKey()}>
          Lock
        </button>
      </div>
      {children}
    </div>
  );
}

export default OperatorGuard;
