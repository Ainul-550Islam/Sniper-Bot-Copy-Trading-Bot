"use client";

import { SupportTicket } from "@/lib/api/support-api";

interface SupportTicketTableProps {
  tickets: SupportTicket[];
}

export function SupportTicketTable({ tickets }: SupportTicketTableProps) {
  if (tickets.length === 0) {
    return (
      <div className="card" style={{ textAlign: "center", padding: "2.5rem" }}>
        <p style={{ margin: 0, color: "var(--muted)" }}>No support tickets filed for this tenant.</p>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Active Support Tickets ({tickets.length})</h3>
      </div>

      <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
            <th style={{ padding: "0.75rem 1rem" }}>Subject</th>
            <th style={{ padding: "0.75rem 1rem" }}>Priority</th>
            <th style={{ padding: "0.75rem 1rem" }}>Status</th>
            <th style={{ padding: "0.75rem 1rem" }}>SLA Target</th>
            <th style={{ padding: "0.75rem 1rem" }}>Created Date</th>
          </tr>
        </thead>
        <tbody>
          {tickets.map((t) => (
            <tr key={t.id} style={{ borderTop: "1px solid var(--line)" }}>
              <td style={{ padding: "0.75rem 1rem" }}>
                <strong>{t.subject}</strong>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>ID: {t.id}</div>
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>
                <span
                  className={`badge ${
                    t.priority === "urgent" || t.priority === "high" ? "badge-bad" : "badge"
                  }`}
                >
                  {t.priority.toUpperCase()}
                </span>
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>
                <span
                  className={`badge ${
                    t.status === "resolved" || t.status === "closed" ? "badge-ok" : "badge-warn"
                  }`}
                >
                  {t.status.toUpperCase()}
                </span>
              </td>
              <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                &lt; {t.sla_target_response_hours} hour SLA
              </td>
              <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                {new Date(t.created_at).toLocaleString()}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default SupportTicketTable;
