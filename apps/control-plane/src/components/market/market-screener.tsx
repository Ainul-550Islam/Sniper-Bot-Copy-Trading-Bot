"use client";

import { useState } from "react";
import { MarketTicker, changePct, priceUsd, venueLabel } from "@/lib/api/market-api";
import { formatUsdCents } from "@/lib/formatters/financial";

interface MarketScreenerProps {
  markets: MarketTicker[];
  loading: boolean;
  onRefresh: () => void;
}

export default function MarketScreener({ markets, loading, onRefresh }: MarketScreenerProps) {
  const [filterVenue, setFilterVenue] = useState<string>("all");
  const [search, setSearch] = useState("");

  const filtered = markets.filter((m) => {
    if (filterVenue !== "all" && m.venue !== filterVenue) return false;
    if (search.trim()) {
      const q = search.toLowerCase();
      return (
        m.symbol.toLowerCase().includes(q) ||
        m.name.toLowerCase().includes(q) ||
        m.venue.toLowerCase().includes(q)
      );
    }
    return true;
  });

  // Distinct venues present in the live feed drive the filter — the UI never
  // offers a venue the server did not report.
  const venues = Array.from(new Set(markets.map((m) => m.venue))).sort();

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          padding: "1rem",
          flexWrap: "wrap",
          gap: "1rem",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: "1rem", flex: 1, minWidth: "280px" }}>
          <input
            type="text"
            placeholder="Search pair, symbol, or name..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="input"
            style={{ width: "100%", maxWidth: "320px" }}
          />

          <select
            value={filterVenue}
            onChange={(e) => setFilterVenue(e.target.value)}
            className="select"
            style={{ minWidth: "160px" }}
          >
            <option value="all">All Venues</option>
            {venues.map((venue) => (
              <option key={venue} value={venue}>
                {venueLabel(venue)}
              </option>
            ))}
          </select>
        </div>

        <button onClick={onRefresh} className="btn btn-secondary" style={{ fontSize: "0.8rem", padding: "0.3rem 0.6rem" }}>
          Refresh Feed
        </button>
      </div>

      {loading ? (
        <div style={{ padding: "2rem", textAlign: "center" }}>Scanning market feeds...</div>
      ) : markets.length === 0 ? (
        <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
          No market data is available from the live feeds right now.
        </div>
      ) : filtered.length === 0 ? (
        <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
          No market pairs matching the active screener filters.
        </div>
      ) : (
        <div style={{ overflowX: "auto" }}>
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Market / Asset</th>
                <th style={{ padding: "0.75rem 1rem" }}>Venue</th>
                <th style={{ padding: "0.75rem 1rem" }}>Price (USD)</th>
                <th style={{ padding: "0.75rem 1rem" }}>24h Change</th>
                <th style={{ padding: "0.75rem 1rem" }}>24h Volume</th>
                <th style={{ padding: "0.75rem 1rem" }}>Liquidity</th>
                <th style={{ padding: "0.75rem 1rem" }}>Modules</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((m) => {
                const pct = changePct(m);
                const isUp = pct >= 0;
                const price = priceUsd(m);
                return (
                  <tr key={m.id} style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <strong>{m.symbol}</strong>
                      <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>{m.name}</div>
                    </td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <span className="badge" style={{ background: "rgba(255,255,255,0.06)" }}>
                        {venueLabel(m.venue)}
                      </span>
                    </td>
                    <td style={{ padding: "0.75rem 1rem", fontFamily: "var(--mono)" }}>
                      ${price < 0.01 ? price.toFixed(6) : price.toFixed(2)}
                    </td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <span style={{ color: isUp ? "var(--ok)" : "var(--bad)", fontWeight: 600 }}>
                        {isUp ? "+" : ""}{pct.toFixed(2)}%
                      </span>
                    </td>
                    <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                      {formatUsdCents(m.volume_24h_usd_cents)}
                    </td>
                    <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                      {formatUsdCents(m.liquidity_usd_cents)}
                    </td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <div style={{ display: "flex", gap: "0.25rem", flexWrap: "wrap" }}>
                        {m.compatible_modules.map((mod) => (
                          <span
                            key={mod}
                            className="badge"
                            style={{ background: "var(--accent-glow)", color: "var(--accent)", fontSize: "0.7rem" }}
                          >
                            {mod}
                          </span>
                        ))}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
