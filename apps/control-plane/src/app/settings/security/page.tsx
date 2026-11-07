"use client";

/**
 * Security posture and tenant access-control console.
 *
 * The initial state is loaded from the authenticated tenant endpoint. The
 * page never invents MFA or IP-allowlist state in the browser.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import SecurityForm from "@/components/settings/security-form";
import CurrentSessionPanel from "@/components/settings/CurrentSessionPanel";
import { ErrorState } from "@/components/common/ErrorState";
import { getSecurityStatus, SecurityPosture } from "@/lib/api/security-api";
import { sessionStore } from "@/lib/auth";

export default function SecuritySettingsPage() {
  const [security, setSecurity] = useState<SecurityPosture | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setSecurity(null);
    setLoading(true);
    setError(null);
  }), []);

  const loadSecurity = useCallback(async () => {
    const requestId = ++requestVersion.current;
    setSecurity(null);
    setLoading(true);
    setError(null);
    if (!selectedOrganizationId) {
      setLoading(false);
      return;
    }
    try {
      const reported = await getSecurityStatus();
      if (requestId === requestVersion.current) setSecurity(reported);
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setError(err instanceof Error ? err.message : "Failed to load security posture");
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void loadSecurity();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [loadSecurity, sessionVersion]);

  return (
    <AppShell title="Security Settings">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Security Posture &amp; Access Controls</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Configure tenant security policies, IP CIDR boundaries, and session credential revocation.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadSecurity()} />}
      {loading && <div className="card">Loading tenant security posture...</div>}
      {!loading && !error && security && (
        <SecurityForm
          key={selectedOrganizationId ?? "no-organization"}
          mfaEnforced={security.mfa_enforced}
          mfaConfigured={security.has_totp_configured}
          ipAllowlist={security.ip_allowlist}
          onRefresh={loadSecurity}
        />
      )}
      <CurrentSessionPanel />
    </AppShell>
  );
}
