"use client";

import { StatusComponent } from "@/lib/api/status-api";

interface ServiceStatusGridProps {
  components: StatusComponent[];
}

export function ServiceStatusGrid({ components }: ServiceStatusGridProps) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(320px, 1fr))", gap: "1rem" }}>
      {components.map((c) => (
        <div key={c.name} className="card">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "0.5rem" }}>
            <div>
              <span style={{ fontSize: "0.75rem", color: "var(--muted)", textTransform: "uppercase" }}>
                {c.category}
              </span>
              <h3 style={{ margin: "0.2rem 0 0", fontSize: "1rem" }}>{c.name}</h3>
            </div>
            <span className={`badge ${c.status === "operational" ? "badge-ok" : "badge-bad"}`}>
              {c.status.toUpperCase()}
            </span>
          </div>

          <p style={{ fontSize: "0.85rem", color: "var(--muted)", margin: "0.5rem 0 0.75rem" }}>
            {c.details}
          </p>

          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              fontSize: "0.75rem",
              color: "var(--muted)",
              borderTop: "1px solid var(--line)",
              paddingTop: "0.5rem",
            }}
          >
            <span>Latency: {c.latency_ms ? `${c.latency_ms}ms` : "—"}</span>
            <span>Checked: {new Date(c.last_checked).toLocaleTimeString()}</span>
          </div>
        </div>
      ))}
    </div>
  );
}

export default ServiceStatusGrid;
