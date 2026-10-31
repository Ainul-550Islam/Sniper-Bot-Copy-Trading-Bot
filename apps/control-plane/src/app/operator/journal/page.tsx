"use client";

import { useEffect, useState, useCallback } from "react";
import { JournalTable } from "@/components/operator/JournalTable";
import { getJournalStatus, type JournalStatus } from "@/lib/api/operator-api";

export default function OperatorJournalPage() {
  const [status, setStatus] = useState<JournalStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadJournal = useCallback(async () => {
    try {
      const res = await getJournalStatus();
      setStatus(res);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadJournal());
  }, [loadJournal]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Intent Journal Storage (/api/journal)</h1>
          <p className="muted">
            Inspect raw JSONL transaction, position, and event journals.
            Supports atomic archiving and rotation without system downtime.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadJournal()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {!loading && !error && status && <JournalTable status={status} onRefresh={loadJournal} />}
    </article>
  );
}
