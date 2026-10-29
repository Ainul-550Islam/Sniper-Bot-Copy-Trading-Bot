"use client";

import { useEffect, useState, useCallback } from "react";
import { FailedTable, type FailedItem } from "@/components/operator/FailedTable";
import { getRecoveryFailed } from "@/lib/api/operator-api";

export default function OperatorRecoveryPage() {
  const [items, setItems] = useState<FailedItem[]>([]);
  const [available, setAvailable] = useState<boolean>(true);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadRecovery = useCallback(async () => {
    try {
      const res = await getRecoveryFailed();
      setItems((res.items as FailedItem[]) || []);
      setAvailable(Boolean(res.available));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadRecovery());
  }, [loadRecovery]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Execution Recovery Queue (/api/recovery/failed)</h1>
          <p className="muted">
            Dead-letter queue tracking transaction intents that failed on-chain or timed out during flight.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadRecovery()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      <FailedTable items={items} available={available} />
    </article>
  );
}
