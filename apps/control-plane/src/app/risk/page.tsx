"use client";

/**
 * Tenant Risk Management & Emergency Controls Dashboard (THIRD.md §102).
 *
 * Visualizes risk limit utilization, drawdown metrics, daily loss guards,
 * and emergency multi-venue kill-switch controls.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import KillSwitchPanel from "@/components/risk/KillSwitchPanel";
import RiskLimitPanel from "@/components/risk/RiskLimitPanel";
import { ErrorState } from "@/components/common/ErrorState";
import { RiskDashboardState, getRiskDashboard } from "@/lib/api/risk-api";

export default function RiskPage() {
  const [risk, setRisk] = useState<RiskDashboardState | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadRisk = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getRiskDashboard();
      setRisk(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load risk posture");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadRisk();
  }, [loadRisk]);

  return (
    <AppShell title="Risk Governance">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Risk Governance &amp; Safeguards</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Monitor tenant-configured safeguards and manage the organization-scoped emergency trading stop.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadRisk()} />}

      {loading ? (
        <div className="card">Loading risk safeguards...</div>
      ) : risk ? (
        <div>
          <KillSwitchPanel killSwitchActive={risk.kill_switch_active} onRefresh={() => void loadRisk()} />
          <RiskLimitPanel rules={risk.rules} referenceAsset={risk.reference_asset} />
        </div>
      ) : null}
    </AppShell>
  );
}
