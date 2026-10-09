"use client";

import { MarketTicker, changePct, priceUsd, venueLabel } from "@/lib/api/market-api";
import { formatPercentage, formatUsdCents } from "@/lib/formatters/financial";

interface MarketDetailPanelProps {
  market: MarketTicker;
}

export function MarketDetailPanel({ market }: MarketDetailPanelProps) {
  const pct = changePct(market);
  const isUp = pct >= 0;
  const price = priceUsd(market);

  return (
    <div className="card" style={{ marginBottom: "1.5rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "1rem" }}>
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
            <h2 style={{ margin: 0 }}>{market.symbol}</h2>
            <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
              {venueLabel(market.venue)}
            </span>
            <span className={`badge ${market.is_active ? "badge-ok" : "badge-warn"}`}>
              {market.is_active ? "ACTIVE TRADING" : "OFFLINE"}
            </span>
          </div>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>{market.name}</p>
        </div>

        <div style={{ textAlign: "right" }}>
          <div style={{ fontSize: "1.5rem", fontWeight: 700, fontFamily: "var(--mono)" }}>
            ${price < 0.01 ? price.toFixed(6) : price.toFixed(2)}
          </div>
          <div style={{ fontSize: "0.85rem", color: isUp ? "var(--ok)" : "var(--bad)", fontWeight: 600 }}>
            {formatPercentage(pct, { showSign: true })} 24h
          </div>
        </div>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))", gap: "1rem", marginTop: "1rem" }}>
        <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>24h Volume</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, fontFamily: "var(--mono)", marginTop: "0.25rem" }}>
            {formatUsdCents(market.volume_24h_usd_cents)}
          </div>
        </div>

        <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Liquidity Depth</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, fontFamily: "var(--mono)", marginTop: "0.25rem" }}>
            {formatUsdCents(market.liquidity_usd_cents)}
          </div>
        </div>

        <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Base Asset</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, marginTop: "0.25rem" }}>
            {market.base_asset}
          </div>
        </div>

        <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Quote Asset</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, marginTop: "0.25rem" }}>
            {market.quote_asset}
          </div>
        </div>

        <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Feed Updated</div>
          <div style={{ fontSize: "0.95rem", fontWeight: 600, marginTop: "0.25rem" }}>
            {market.updated_at}
          </div>
        </div>
      </div>
    </div>
  );
}

export default MarketDetailPanel;
