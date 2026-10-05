"use client";

import { RiskLimitRule } from "@/lib/api/risk-api";
import { formatUsdCents } from "@/lib/formatters/financial";

interface RiskLimitPanelProps {
  rules: RiskLimitRule[];
}

export function RiskLimitPanel({ rules }: RiskLimitPanelProps) {
  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Active Risk Limit Safeguards</h3>
      </div>

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
            return (
              <tr key={rule.id} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <strong>{rule.name}</strong>
                  <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>{rule.id}</div>
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <span className="badge" style={{ textTransform: "uppercase" }}>{rule.scope}</span>
                </td>
                <td style={{ padding: "0.75rem 1rem", fontFamily: "var(--mono)" }}>
                  {formatUsdCents(rule.limit_usd_cents)}
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
                    <div
                      style={{
                        flex: 1,
                        height: "6px",
                        background: "rgba(255,255,255,0.1)",
                        borderRadius: "3px",
                        overflow: "hidden",
                        minWidth: "60px",
                      }}
                    >
                      <div
                        style={{
                          height: "100%",
                          width: `${Math.min(100, rule.utilization_pct)}%`,
                          background: isBreached ? "var(--bad)" : isWarning ? "var(--warn)" : "var(--ok)",
                        }}
                      />
                    </div>
                    <span style={{ fontSize: "0.8rem", fontFamily: "var(--mono)" }}>{rule.utilization_pct.toFixed(1)}%</span>
                  </div>
                </td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <span
                    className={`badge ${
                      isBreached ? "badge-bad" : isWarning ? "badge-warn" : "badge-ok"
                    }`}
                  >
                    {rule.status.toUpperCase()}
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default RiskLimitPanel;
