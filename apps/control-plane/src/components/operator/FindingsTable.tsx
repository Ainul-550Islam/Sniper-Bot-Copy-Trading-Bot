"use client";

import type { ReconciliationFinding } from "@/lib/api/operator-api";

export interface FindingsTableProps {
  findings: ReconciliationFinding[];
}

export function FindingsTable({ findings }: FindingsTableProps) {
  return (
    <div className="card" style={{ padding: 0, overflowX: "auto" }}>
      <table className="table" style={{ width: "100%", margin: 0 }}>
        <thead>
          <tr>
            <th>Finding ID</th>
            <th>Kind</th>
            <th>Module</th>
            <th>Venue</th>
            <th>Asset</th>
            <th>Expected</th>
            <th>Actual</th>
            <th>Detail</th>
          </tr>
        </thead>
        <tbody>
          {findings.length === 0 ? (
            <tr>
              <td colSpan={8} className="muted" style={{ textAlign: "center", padding: "2rem" }}>
                No active reconciliation findings. All ledgers in sync.
              </td>
            </tr>
          ) : (
            findings.map((f) => (
              <tr key={f.finding_id}>
                <td>
                  <code>{f.finding_id.slice(0, 16)}…</code>
                </td>
                <td>
                  <span className="tag tag--danger">{f.kind}</span>
                </td>
                <td>{f.module || "—"}</td>
                <td>{f.venue || "—"}</td>
                <td><strong>{f.asset || "—"}</strong></td>
                <td>{f.expected}</td>
                <td style={{ color: "var(--bad)", fontWeight: 600 }}>{f.actual}</td>
                <td className="small">{f.detail}</td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}

export default FindingsTable;
