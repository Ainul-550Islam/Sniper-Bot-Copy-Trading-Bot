"use client";

/**
 * In-Product API & SDK Integration Documentation (THIRD.md §110).
 */

import AppShell from "@/components/AppShell";
import ApiExplorer from "@/components/docs/ApiExplorer";

export default function DocsPage() {
  return (
    <AppShell title="Developer Documentation">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Developer API &amp; SDK Documentation</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Production REST, WebSocket, and SDK integration contracts for quantitative trading teams.
        </p>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: "1rem", marginBottom: "1.5rem" }}>
        <div className="card">
          <h3 style={{ marginTop: 0 }}>OpenAPI 3.1 Contract</h3>
          <p style={{ fontSize: "0.85rem", color: "var(--muted)" }}>
            Download the complete machine-readable schema covering multi-tenant trading data plane and SaaS endpoints.
          </p>
          <a href="/api/saas/openapi.json" target="_blank" className="btn btn-secondary" style={{ fontSize: "0.8rem" }}>
            View openapi.json
          </a>
        </div>

        <div className="card">
          <h3 style={{ marginTop: 0 }}>Rust SaaS SDK</h3>
          <p style={{ fontSize: "0.85rem", color: "var(--muted)" }}>
            Type-safe client crate with exact integer arithmetic, backtest execution, and custody management.
          </p>
          <code style={{ fontSize: "0.8rem", display: "block", marginBottom: "0.75rem" }}>
            cargo add sniper-saas-sdk
          </code>
        </div>

        <div className="card">
          <h3 style={{ marginTop: 0 }}>Authentication</h3>
          <p style={{ fontSize: "0.85rem", color: "var(--muted)" }}>
            Pass API key or JWT bearer token in HTTP header:
          </p>
          <code style={{ fontSize: "0.75rem", display: "block" }}>
            Authorization: Bearer snpr_live_...
          </code>
        </div>
      </div>

      <ApiExplorer />
    </AppShell>
  );
}
