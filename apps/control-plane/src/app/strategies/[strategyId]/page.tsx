"use client";

/**
 * Strategy Detail & Version History Page (THIRD.md §103).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import StrategyVersionHistory from "@/components/strategy/StrategyVersionHistory";
import { ErrorState } from "@/components/common/ErrorState";
import { StrategyRecord, getStrategy } from "@/lib/api/strategy-api";

export default function StrategyDetailPage() {
  const params = useParams();
  const strategyId = typeof params?.strategyId === "string" ? params.strategyId : "";
  const [strategy, setStrategy] = useState<StrategyRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestVersion = useRef(0);

  const load = useCallback(async () => {
    const requestId = ++requestVersion.current;
    if (!strategyId) {
      setStrategy(null);
      setError("Strategy identifier is missing.");
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    setStrategy(null);
    try {
      const data = await getStrategy(strategyId);
      if (requestId === requestVersion.current) setStrategy(data);
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setStrategy(null);
        setError(err instanceof Error ? err.message : "Failed to load strategy details");
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, [strategyId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load]);

  return (
    <AppShell title="Strategy Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Strategy Details: {strategy?.name || strategyId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Immutable version provenance, parameter audit trail, and execution state.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading strategy details...</div>
      ) : strategy ? (
        <div>
          <StrategyVersionHistory strategy={strategy} />
        </div>
      ) : null}
    </AppShell>
  );
}
