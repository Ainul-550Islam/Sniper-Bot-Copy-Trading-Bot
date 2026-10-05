"use client";

/**
 * Security Posture & Hardening Console (SECOND.md §59).
 *
 * Enforce multi-factor authentication, IP allowlists, KMS custody key rotation,
 * and emergency session invalidation.
 */

import { useState } from "react";
import AppShell from "@/components/AppShell";
import SecurityForm from "@/components/settings/security-form";

export default function SecuritySettingsPage() {
  const [mfaEnforced] = useState(true);
  const [ipAllowlist] = useState(["198.51.100.0/24", "203.0.113.45/32"]);

  return (
    <AppShell title="Security Settings">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Security Posture &amp; Access Controls</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Configure enterprise hardening policies, IP CIDR boundaries, and emergency session revocation.
        </p>
      </div>

      <SecurityForm
        mfaEnforced={mfaEnforced}
        ipAllowlist={ipAllowlist}
        onRefresh={() => {}}
      />
    </AppShell>
  );
}
