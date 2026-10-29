"use client";

import { useEffect, useState, useCallback } from "react";
import { AuditTable } from "@/components/operator/AuditTable";
import { getAuditTrail, type AuditRecord } from "@/lib/api/operator-api";

export default function OperatorAuditPage() {
  const [records, setRecords] = useState<AuditRecord[]>([]);
  const [durable, setDurable] = useState<boolean>(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadAudit = useCallback(async () => {
    try {
      const res = await getAuditTrail(100);
      setRecords(res.records || []);
      setDurable(Boolean(res.durable));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadAudit());
  }, [loadAudit]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Immutable Audit Trail (/api/audit)</h1>
          <p className="muted">
            Append-only decision log recording every granted and denied mutating action.
            Chained cryptographic hashes ensure tamper-evident operation.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadAudit()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      <AuditTable records={records} durable={durable} onRefresh={loadAudit} />
    </article>
  );
}
