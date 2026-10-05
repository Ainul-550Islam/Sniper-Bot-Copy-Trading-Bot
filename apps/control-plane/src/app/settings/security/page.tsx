"use client";

/**
 * Security posture and tenant access-control console.
 *
 * The initial state is loaded from the authenticated tenant endpoint. The
 * page never invents MFA or IP-allowlist state in the browser.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import SecurityForm from "@/components/settings/security-form";
import { ErrorState } from "@/components/common/ErrorState";
import { getSecurityStatus, SecurityPosture } from "@/lib/api/security-api";

export default function SecuritySettingsPage() {
  const [security, setSecurity] = useState<SecurityPosture | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadSecurity = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setSecurity(await getSecurityStatus());
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load security posture");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSecurity();
  }, [loadSecurity]);

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
          mfaEnforced={security.mfa_enforced}
          mfaConfigured={security.has_totp_configured}
          ipAllowlist={security.ip_allowlist}
          onRefresh={loadSecurity}
        />
      )}
    </AppShell>
  );
}
