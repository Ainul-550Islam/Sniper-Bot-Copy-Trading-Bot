"use client";

import { useState } from "react";
import {
  verifyAuditChain,
  type AuditRecord,
  type AuditVerifyResponse,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface AuditTableProps {
  records: AuditRecord[];
  durable: boolean;
  onRefresh?: () => Promise<void> | void;
}

export function AuditTable({ records, durable, onRefresh }: AuditTableProps) {
  const [verifying, setVerifying] = useState(false);
  const [verifyResult, setVerifyResult] = useState<AuditVerifyResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleVerify = async () => {
    setVerifying(true);
    setError(null);
    try {
      const res = await verifyAuditChain();
      setVerifyResult(res);
      await onRefresh?.();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setVerifying(false);
    }
  };

  return (
    <div className="stack">
      <div className="row-between">
        <div>
          <h3>Audit Trail &amp; Cryptographic Hash Chain</h3>
          <p className="muted small">
            Every mutating decision is recorded with cryptographic SHA-256 chaining.
            Persistence durability: <strong>{durable ? "Database Backed" : "Memory Only"}</strong>.
          </p>
        </div>
        <div className="row">
          <button
            type="button"
            className="primary"
            disabled={verifying}
            onClick={() => void handleVerify()}
          >
            {verifying ? "Verifying Chain…" : "Verify Chain (/api/audit/verify)"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {verifyResult && (
        <div
          className="card"
          style={{
            borderLeft: `4px solid ${
              verifyResult.status === "intact" ? "var(--good)" : "var(--bad)"
            }`,
          }}
        >
          <div className="row-between">
            <div>
              <h4 style={{ margin: 0 }}>
                Chain Status:{" "}
                <span
                  className={`tag ${
                    verifyResult.status === "intact" ? "tag--active" : "tag--danger"
                  }`}
                >
                  {verifyResult.status.toUpperCase()}
                </span>
              </h4>
              <p className="small muted" style={{ margin: "0.3rem 0 0" }}>
                {verifyResult.status === "intact" &&
                  `Cryptographic hash chain is intact across ${verifyResult.entries ?? records.length} records.`}
                {verifyResult.status === "broken" &&
                  `Tampering detected! Chain broken at entry id: ${verifyResult.at_id}.`}
                {verifyResult.status === "not_chained" &&
                  "Durable database audit trail is not active in this environment."}
                {verifyResult.status === "unknown" &&
                  `Verification could not be completed: ${verifyResult.error ?? "unknown error"}.`}
              </p>
            </div>
          </div>
        </div>
      )}

      <div className="card" style={{ padding: 0, overflowX: "auto" }}>
        <table className="table" style={{ width: "100%", margin: 0 }}>
          <thead>
            <tr>
              <th>ID</th>
              <th>Timestamp</th>
              <th>Actor</th>
              <th>Action</th>
              <th>Target</th>
              <th>Outcome</th>
              <th>Hash</th>
            </tr>
          </thead>
          <tbody>
            {records.length === 0 ? (
              <tr>
                <td colSpan={7} className="muted" style={{ textAlign: "center", padding: "2rem" }}>
                  No audit records logged yet.
                </td>
              </tr>
            ) : (
              records.map((r) => (
                <tr key={r.id}>
                  <td><code>#{r.id}</code></td>
                  <td className="small muted">{new Date(r.ts).toLocaleString()}</td>
                  <td><strong>{r.actor}</strong></td>
                  <td><code>{r.action}</code></td>
                  <td className="small muted">{r.target || "—"}</td>
                  <td>
                    <span
                      className={`tag ${
                        r.outcome === "success" || r.outcome === "grant"
                          ? "tag--active"
                          : "tag--danger"
                      }`}
                    >
                      {r.outcome}
                    </span>
                  </td>
                  <td>
                    <code className="small">{r.hash ? r.hash.slice(0, 16) + "…" : "—"}</code>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export default AuditTable;
