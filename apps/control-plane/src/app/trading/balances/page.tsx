"use client";

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { BalancesTable } from "@/components/trading/BalancesTable";
import { toDisplayError } from "@/lib/api";
import { tradingDetailApi, type BalancesResponse } from "@/lib/api/trading-detail-api";

export default function BalancesPage() {
  const [balances, setBalances] = useState<BalancesResponse["items"]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await tradingDetailApi.balances();
      setBalances(Array.isArray(response.items) ? response.items : []);
    } catch (err: unknown) {
      setBalances([]);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

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
