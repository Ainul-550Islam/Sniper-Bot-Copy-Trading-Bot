"use client";

import { useEffect, useState, useCallback } from "react";
import { FindingsTable } from "@/components/operator/FindingsTable";
import {
  getReconciliationFindings,
  type ReconciliationFinding,
} from "@/lib/api/operator-api";

export default function OperatorReconciliationPage() {
  const [findings, setFindings] = useState<ReconciliationFinding[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadFindings = useCallback(async () => {
    try {
      const res = await getReconciliationFindings(100);
      if (!Array.isArray(res.findings)) throw new Error("Invalid reconciliation findings response");
      setFindings(res.findings);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadFindings());
  }, [loadFindings]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Accounting Reconciliation (/api/reconciliation/findings)</h1>
          <p className="muted">
            Continuous reconciliation between internal trade ledgers, DEX on-chain settlement, and custody balances.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadFindings()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {!loading && !error && <FindingsTable findings={findings} />}
    </article>
  );
}
