"use client";

import { RiskLimitRule } from "@/lib/api/risk-api";

interface RiskLimitPanelProps {
  rules: RiskLimitRule[];
  referenceAsset?: string;
}

export function RiskLimitPanel({ rules, referenceAsset = "reference units" }: RiskLimitPanelProps) {
  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Active Risk Limit Safeguards</h3>
        <p style={{ margin: "0.35rem 0 0", color: "var(--muted)", fontSize: "0.8rem" }}>
          Values are reported in the risk engine reference asset: {referenceAsset}.
        </p>
      </div>

      {rules.length === 0 ? (
        <p style={{ padding: "1rem", color: "var(--muted)" }}>No risk ceilings are configured in the authoritative risk engine.</p>
      ) : (
        <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Safeguard Rule</th>
              <th style={{ padding: "0.75rem 1rem" }}>Scope</th>
              <th style={{ padding: "0.75rem 1rem" }}>Configured Ceiling</th>
              <th style={{ padding: "0.75rem 1rem" }}>Current Utilization</th>
              <th style={{ padding: "0.75rem 1rem" }}>Status</th>
            </tr>
          </thead>
          <tbody>
            {rules.map((rule) => {
              const isWarning = rule.status === "warning";
              const isBreached = rule.status === "breached";
              const pct = rule.utilization_pct ?? 0;
              return (
                <tr key={rule.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <strong>{rule.name}</strong>
                    <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>{rule.id}</div>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{rule.scope}</span></td>
                  <td style={{ padding: "0.75rem 1rem", fontFamily: "var(--mono)" }}>
                    {rule.limit_ref === null ? "Not configured" : `${rule.limit_ref.toLocaleString()} ${referenceAsset}`}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    {rule.current_utilization_ref === null ? "Not measured" : (
                      <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                        <div style={{ flex: 1, height: "6px", background: "rgba(255,255,255,0.1)", borderRadius: "3px", overflow: "hidden", minWidth: "60px" }}>
                          <div style={{ height: "100%", width: `${Math.min(100, Math.max(0, pct))}%`, background: isBreached ? "var(--bad)" : isWarning ? "var(--warn)" : "var(--ok)" }} />
                        </div>
                        <span style={{ fontSize: "0.8rem", fontFamily: "var(--mono)" }}>{rule.current_utilization_ref.toLocaleString()} ({pct.toFixed(1)}%)</span>
                      </div>
                    )}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className={`badge ${isBreached ? "badge-bad" : isWarning ? "badge-warn" : "badge-ok"}`}>
                      {rule.status.toUpperCase()}
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

export default RiskLimitPanel;
