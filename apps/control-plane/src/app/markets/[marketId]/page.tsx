"use client";

/**
 * Market Detail Page (THIRD.md §106).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import MarketDetailPanel from "@/components/market/MarketDetailPanel";
import { ErrorState } from "@/components/common/ErrorState";
import { MarketTicker, getMarket } from "@/lib/api/market-api";

export default function MarketDetailPage() {
  const params = useParams();
  const marketId = typeof params?.marketId === "string" ? params.marketId : "";
  const [market, setMarket] = useState<MarketTicker | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestVersion = useRef(0);

  const load = useCallback(async () => {
    const version = ++requestVersion.current;
    if (!marketId) {
      setMarket(null);
      setError("A market ID is required.");
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    setMarket(null);
    try {
      const data = await getMarket(marketId);
      if (requestVersion.current === version) setMarket(data);
    } catch (err: unknown) {
      if (requestVersion.current === version) {
        setMarket(null);
        setError(err instanceof Error ? err.message : "Failed to load market pair data");
      }
    } finally {
      if (requestVersion.current === version) setLoading(false);
    }
  }, [marketId]);

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
    <AppShell title="Market Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Market Pair: {market?.symbol || marketId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Live order book depth, 24-hour volume metrics, and execution routing eligibility.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading market data...</div>
      ) : market ? (
        <MarketDetailPanel market={market} />
      ) : null}
    </AppShell>
  );
}
