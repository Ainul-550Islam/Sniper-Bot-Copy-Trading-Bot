"use client";

import { useState } from "react";

export function ApiExplorer() {
  const [selectedEndpoint, setSelectedEndpoint] = useState("GET /api/saas/portfolio");

  const endpoints = [
    {
      method: "GET",
      path: "/api/saas/portfolio",
      description: "Fetches authoritative multi-venue portfolio equity and exposures.",
      responseExample: JSON.stringify(
        {
          organization_id: "",
          total_equity_usd_cents: 0,
          available_cash_usd_cents: 0,
          allocated_margin_usd_cents: 0,
          unrealized_pnl_usd_cents: 0,
          as_of: "",
        },
        null,
        2,
      ),
    },
    {
      method: "GET",
      path: "/api/tenant/strategies",
      description: "Lists all trading strategies configured for the tenant.",
      responseExample: JSON.stringify(
        {
          items: [],
        },
        null,
        2,
      ),
    },
    {
      method: "POST",
      path: "/api/saas/risk-dashboard/kill-switch",
      description: "Activates or deactivates the deployment risk-engine kill switch when the authenticated organization owns that engine.",
      responseExample: JSON.stringify(
        {
          success: true,
          kill_switch_active: true,
          updated_at: "2026-10-03T12:00:00Z",
        },
        null,
        2,
      ),
    },
  ];

  const current =
    endpoints.find((e) => `${e.method} ${e.path}` === selectedEndpoint) ?? endpoints[0]!;

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Interactive API Contract Explorer</h3>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 0 }}>
        <div style={{ borderRight: "1px solid var(--line)", padding: "1rem" }}>
          <div style={{ fontSize: "0.8rem", color: "var(--muted)", marginBottom: "0.5rem" }}>
            Select Endpoint Route
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
            {endpoints.map((e) => {
              const key = `${e.method} ${e.path}`;
              const active = selectedEndpoint === key;
              return (
                <button
                  key={key}
                  onClick={() => setSelectedEndpoint(key)}
                  className={`btn ${active ? "btn-primary" : "btn-secondary"}`}
                  style={{
                    textAlign: "left",
                    fontSize: "0.8rem",
                    justifyContent: "flex-start",
                    fontFamily: "var(--mono)",
                  }}
                >
                  <span style={{ fontWeight: 700, marginRight: "0.4rem" }}>{e.method}</span>
                  {e.path}
                </button>
              );
            })}
          </div>
        </div>

        <div style={{ padding: "1rem" }}>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginBottom: "0.5rem" }}>
            <span className="badge badge-ok">{current.method}</span>
            <code style={{ fontSize: "0.9rem" }}>{current.path}</code>
          </div>
          <p style={{ fontSize: "0.85rem", color: "var(--muted)", margin: "0 0 1rem" }}>
            {current.description}
          </p>

          <div style={{ fontSize: "0.8rem", color: "var(--muted)", marginBottom: "0.25rem" }}>
            Sample Response Payload (JSON)
          </div>
          <pre
            style={{
              background: "rgba(0,0,0,0.4)",
              padding: "0.75rem",
              borderRadius: "6px",
              fontFamily: "var(--mono)",
              fontSize: "0.8rem",
              overflowX: "auto",
              margin: 0,
            }}
          >
            {current.responseExample}
          </pre>
        </div>
      </div>
    </div>
  );
}

export default ApiExplorer;
