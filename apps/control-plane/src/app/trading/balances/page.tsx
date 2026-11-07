"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { BalancesTable } from "@/components/trading/BalancesTable";
import { toDisplayError } from "@/lib/api";
import { sessionStore } from "@/lib/auth";
import { tradingDetailApi, type BalancesResponse } from "@/lib/api/trading-detail-api";

export default function BalancesPage() {
  const [balances, setBalances] = useState<BalancesResponse["items"]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sessionVersion, setSessionVersion] = useState(0);
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setBalances([]);
    setLoading(true);
    setError(null);
  }), []);

  const load = useCallback(async () => {
    const requestId = ++requestVersion.current;
    setLoading(true);
    setError(null);
    setBalances([]);
    try {
      const response = await tradingDetailApi.balances();
      if (requestId === requestVersion.current) {
        setBalances(Array.isArray(response.items) ? response.items : []);
      }
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setBalances([]);
        setError(toDisplayError(err));
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load, sessionVersion]);

  return (
    <AppShell title="Balances">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Balances</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Tenant balances as returned by the trading data plane. Values are
          server truth and are never estimated client-side.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading balances…</div>
      ) : (
        <BalancesTable balances={balances} />
      )}
    </AppShell>
  );
}
