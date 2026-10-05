"use client";

import { StrategyRecord } from "@/lib/api/strategy-api";

interface StrategyVersionHistoryProps {
  strategy: StrategyRecord;
}

export function StrategyVersionHistory({ strategy }: StrategyVersionHistoryProps) {
  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Version &amp; Audit Provenance</h3>
      </div>

      <div style={{ padding: "1rem" }}>
        <div style={{ display: "flex", alignItems: "flex-start", gap: "1rem", marginBottom: "1rem" }}>
          <div
            style={{
              width: "28px",
              height: "28px",
              borderRadius: "50%",
              background: "var(--accent)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "0.8rem",
            }}
          >
            v{strategy.version}
          </div>
          <div style={{ flex: 1 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <strong>Current Active Version ({strategy.status.toUpperCase()})</strong>
              <span style={{ fontSize: "0.75rem", color: "var(--muted)" }}>
                {new Date(strategy.updated_at).toLocaleString()}
              </span>
            </div>
            <p style={{ margin: "0.25rem 0 0.5rem", fontSize: "0.85rem", color: "var(--muted)" }}>
              {strategy.description || "Production trading configuration parameters applied."}
            </p>
            <pre
              style={{
                background: "rgba(0,0,0,0.3)",
                padding: "0.5rem",
                borderRadius: "4px",
                fontSize: "0.75rem",
                fontFamily: "var(--mono)",
                margin: 0,
              }}
            >
              {JSON.stringify(strategy.parameters, null, 2)}
            </pre>
          </div>
        </div>
      </div>
    </div>
  );
}

export default StrategyVersionHistory;
