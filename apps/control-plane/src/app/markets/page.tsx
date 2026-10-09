"use client";

/**
 * Market Discovery & Live Screener Page (GAP MAP v2, Part 5 wiring).
 *
 * Live discovery feed for Solana DEX pairs (Raydium, Pump.fun/PumpSwap) and
 * Polymarket CLOB markets, served by the tenant trading data plane. The page
 * renders exactly what the feed reports: rows from `items`, plus feed health
 * and cache-staleness from the same response. A refused or empty feed is an
 * error banner — never a fabricated table.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import MarketScreener from "@/components/market/market-screener";
import { FeedStatus, MarketTicker, listMarkets } from "@/lib/api/market-api";

export default function MarketsPage() {
  const [markets, setMarkets] = useState<MarketTicker[]>([]);
  const [feeds, setFeeds] = useState<FeedStatus[]>([]);
  const [fromCache, setFromCache] = useState(false);
  const [fetchedAt, setFetchedAt] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listMarkets();
      setMarkets(data.items);
      setFeeds(data.feeds);
      setFromCache(data.from_cache);
      setFetchedAt(data.fetched_at);
    } catch (err: unknown) {
      // Honest empty state: the backend answered with a refusal (503
      // market_data_unavailable, plane unavailable, or an auth denial).
      setMarkets([]);
      setFeeds([]);
      setError(err instanceof Error ? err.message : "Failed to load market screener feed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  const degradedFeeds = feeds.filter((feed) => !feed.ok);

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

      {!loading && !error && (degradedFeeds.length > 0 || fromCache) && (
        <div
          className="card"
          style={{ borderColor: "var(--warn)", background: "var(--warn-glow)", marginBottom: "1.5rem", fontSize: "0.85rem" }}
        >
          {fromCache && <p style={{ margin: "0 0 0.25rem" }}>Showing a cached snapshot (fetched {fetchedAt ?? "recently"}); the live feed did not answer in time.</p>}
          {degradedFeeds.map((feed) => (
            <p key={feed.name} style={{ margin: "0 0 0.25rem" }}>
              Feed <strong>{feed.name}</strong> is not returning data{feed.detail ? `: ${feed.detail}` : ""}.
            </p>
          ))}
        </div>
      )}

      <MarketScreener markets={markets} loading={loading} onRefresh={() => void loadData()} />
    </AppShell>
  );
}
