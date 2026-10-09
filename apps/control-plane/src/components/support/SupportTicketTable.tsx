"use client";

import { SupportTicket } from "@/lib/api/support-api";

interface SupportTicketTableProps {
  tickets: SupportTicket[];
}

export default function SupportTicketTable({ tickets }: SupportTicketTableProps) {
  if (tickets.length === 0) {
    return <div className="card" style={{ color: "var(--muted)" }}>No support tickets have been recorded for this organization.</div>;
  }
  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ textAlign: "left" }}>
            <th style={{ padding: "0.75rem 1rem" }}>Subject</th>
            <th style={{ padding: "0.75rem 1rem" }}>Priority</th>
            <th style={{ padding: "0.75rem 1rem" }}>Status</th>
            <th style={{ padding: "0.75rem 1rem" }}>Created</th>
          </tr>
        </thead>
        <tbody>
          {tickets.map((ticket) => (
            <tr key={ticket.id} style={{ borderTop: "1px solid var(--line)" }}>
              <td style={{ padding: "0.75rem 1rem" }}>
                <strong>{ticket.subject}</strong>
                <div style={{ color: "var(--muted)", fontSize: "0.8rem", marginTop: "0.25rem" }}>{ticket.description}</div>
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>
                <span className={`badge ${ticket.priority === "urgent" || ticket.priority === "high" ? "badge-bad" : "badge"}`}>
                  {ticket.priority.toUpperCase()}
                </span>
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>{ticket.status.replace("_", " ")}</td>
              <td style={{ padding: "0.75rem 1rem", color: "var(--muted)" }}>{new Date(ticket.created_at).toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
