"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { ExecutionDetail } from "@/components/trading/ExecutionDetail";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi } from "@/lib/api/trading-detail-api";
import type { TenantExecution } from "@/lib/customer-trading-api";

export default function ExecutionDetailPage() {
  const params = useParams();
  const executionId = typeof params?.executionId === "string" ? params.executionId : "";
  const [execution, setExecution] = useState<TenantExecution | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!executionId) return;
    setLoading(true);
    setError(null);
    try {
      const next = await tradingDetailApi.execution(executionId);
      // A body without an id is not an execution record; surface it honestly.
      if (next && typeof next.id === "string") {
        setExecution(next);
      } else {
        setError(`The server returned no execution record for ${executionId}.`);
      }
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`Execution ${executionId} not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [executionId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Execution Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Execution: {executionId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Authoritative execution record from the tenant trading data plane.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading execution…</div>
      ) : execution ? (
        <ExecutionDetail execution={execution} />
      ) : null}
    </AppShell>
  );
}
