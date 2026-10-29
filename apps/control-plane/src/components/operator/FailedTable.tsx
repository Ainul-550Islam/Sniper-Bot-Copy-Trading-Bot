"use client";

export interface FailedItem {
  id?: string | number;
  intent_id?: string;
  module?: string;
  venue?: string;
  error?: string;
  failed_at?: string;
  attempts?: number;
  [key: string]: unknown;
}

export interface FailedTableProps {
  items: FailedItem[];
  available: boolean;
}

export function FailedTable({ items, available }: FailedTableProps) {
  if (!available) {
    return (
      <div className="card" style={{ textAlign: "center", padding: "2rem" }}>
        <p className="muted" style={{ margin: 0 }}>
          Recovery queue store is currently unavailable or database is unattached.
        </p>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: 0, overflowX: "auto" }}>
      <table className="table" style={{ width: "100%", margin: 0 }}>
        <thead>
          <tr>
            <th>Item / Intent ID</th>
            <th>Module</th>
            <th>Venue</th>
            <th>Error / Cause</th>
            <th>Attempts</th>
            <th>Timestamp</th>
          </tr>
        </thead>
        <tbody>
          {items.length === 0 ? (
            <tr>
              <td colSpan={6} className="muted" style={{ textAlign: "center", padding: "2rem" }}>
                Recovery queue is clear. No failed executions pending recovery.
              </td>
            </tr>
          ) : (
            items.map((item, idx) => {
              const id = String(item.intent_id || item.id || idx);
              const displayError =
                typeof item.error === "string"
                  ? item.error
                  : typeof item.reason === "string"
                  ? item.reason
                  : JSON.stringify(item.error || item.reason || item);
              return (
                <tr key={id}>
                  <td><code>{id}</code></td>
                  <td>{item.module || "—"}</td>
                  <td>{item.venue || "—"}</td>
                  <td className="small" style={{ color: "var(--bad)" }}>
                    {displayError}
                  </td>
                  <td>{item.attempts ?? 1}</td>
                  <td className="small muted">
                    {item.failed_at ? new Date(item.failed_at).toLocaleString() : "—"}
                  </td>
                </tr>
              );
            })
          )}
        </tbody>
      </table>
    </div>
  );
}

export default FailedTable;
