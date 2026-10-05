"use client";

/**
 * Market Discovery & Live Screener Page (SECOND.md §53).
 *
 * Real-time discovery feed for Solana DEX pairs (Raydium v4, CLMM),
 * Pump.fun bonding curves, and Polymarket prediction event order books.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import MarketScreener from "@/components/market/market-screener";
import { MarketTicker, listMarkets } from "@/lib/api/market-api";

export default function MarketsPage() {
  const [markets, setMarkets] = useState<MarketTicker[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listMarkets();
      setMarkets(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load market screener feed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  return (
    <AppShell title="Market Screener">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Market Screener &amp; Liquidity Pools</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Live discovery feed for Solana DEX pairs, newly graduated bonding curves, and prediction event markets.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      <MarketScreener markets={markets} loading={loading} onRefresh={() => void loadData()} />
    </AppShell>
  );
}
