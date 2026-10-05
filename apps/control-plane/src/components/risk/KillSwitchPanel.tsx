"use client";

import { useState } from "react";
import { toggleKillSwitch } from "@/lib/api/risk-api";

interface KillSwitchPanelProps {
  killSwitchActive: boolean;
  onRefresh: () => void;
}

export function KillSwitchPanel({ killSwitchActive, onRefresh }: KillSwitchPanelProps) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleToggle = async () => {
    const nextState = !killSwitchActive;
    const promptMsg = nextState
      ? "EMERGENCY: This will immediately freeze all tenant trading bot execution, cancel open orders, and block new quotes. Confirm activation?"
      : "Resume normal trading execution for this organization?";

    if (!confirm(promptMsg)) return;

    setLoading(true);
    setError(null);
    try {
      await toggleKillSwitch({
        active: nextState,
        reason: nextState ? "Manual operator emergency trigger" : "Manual operator resumption",
      });
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to toggle kill switch");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div
      className="card"
      style={{
        borderColor: killSwitchActive ? "var(--bad)" : "rgba(255,255,255,0.1)",
        background: killSwitchActive ? "var(--bad-glow)" : "var(--panel)",
        marginBottom: "1.5rem",
      }}
    >
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", flexWrap: "wrap", gap: "1rem" }}>
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
            <h3 style={{ margin: 0, color: killSwitchActive ? "var(--bad)" : "var(--text)" }}>
              Emergency Tenant Kill-Switch
            </h3>
            <span className={`badge ${killSwitchActive ? "badge-bad" : "badge-ok"}`}>
              {killSwitchActive ? "ACTIVE — TRADING BLOCKED" : "STANDBY — NORMAL"}
            </span>
          </div>
          <p style={{ margin: "0.25rem 0 0", fontSize: "0.85rem", color: "var(--muted)" }}>
            Instant multi-venue execution circuit-breaker with lease-fence invalidation and atomic order cancellation.
          </p>
        </div>

        <button
          onClick={handleToggle}
          disabled={loading}
          className={`btn ${killSwitchActive ? "btn-primary" : "btn-secondary"}`}
          style={{
            background: killSwitchActive ? "var(--ok)" : "var(--bad)",
            color: "#fff",
            borderColor: killSwitchActive ? "var(--ok)" : "var(--bad)",
          }}
        >
          {loading ? "Processing..." : killSwitchActive ? "Deactivate & Resume Trading" : "ACTIVATE EMERGENCY STOP"}
        </button>
      </div>

      {error && (
        <div style={{ marginTop: "1rem", color: "var(--bad)", fontSize: "0.85rem" }}>
          {error}
        </div>
      )}
    </div>
  );
}

export default KillSwitchPanel;
