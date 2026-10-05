"use client";

/**
 * Polymarket CLOB Strategy Configuration (SECOND.md §56).
 *
 * Automated CTF order book market making, binary event spread capture,
 * and max position exposure limits.
 */

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";

export default function PolymarketConfigPage() {
  const [name, setName] = useState("Election 2024 Spread Capture");
  const [marketSlugs, setMarketSlugs] = useState("presidential-election-winner-2024");
  const [maxPositionUsd, setMaxPositionUsd] = useState(5000);
  const [spreadThresholdBps, setSpreadThresholdBps] = useState(30);
  const [clobOrderType, setClobOrderType] = useState("GTC");
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setStatus(null);

    const slugs = marketSlugs
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);

    try {
      await createStrategy({
        module_family: "polymarket",
        name,
        description: "Polymarket binary prediction outcome market maker & arbitrage strategy",
        parameters: {
          market_slugs: slugs,
          max_position_usd: maxPositionUsd,
          spread_threshold_bps: spreadThresholdBps,
          clob_order_type: clobOrderType,
        },
      });
      setStatus({ message: "Polymarket strategy configuration active!", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save configuration", ok: false });
    } finally {
      setLoading(false);
    }
  };

  return (
    <AppShell title="Polymarket Configuration">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Polymarket CLOB Strategy Configuration</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Configure market-making spread thresholds, max dollar risk, and CTF order routing.
        </p>
      </div>

      {status && (
        <div
          className="card"
          style={{
            color: status.ok ? "var(--ok)" : "var(--bad)",
            background: status.ok ? "var(--ok-glow)" : "var(--bad-glow)",
            marginBottom: "1.5rem",
          }}
        >
          {status.message}
        </div>
      )}

      <div className="card">
        <form onSubmit={handleSave}>
          <div style={{ marginBottom: "1.5rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Strategy Profile Name
            </label>
            <input
              type="text"
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="input"
              style={{ width: "100%", maxWidth: "480px" }}
            />
          </div>

          <div style={{ marginBottom: "1.5rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Target Market Condition Slugs (One per line)
            </label>
            <textarea
              rows={3}
              required
              value={marketSlugs}
              onChange={(e) => setMarketSlugs(e.target.value)}
              placeholder="presidential-election-winner-2024&#10;fed-interest-rates-nov-2024"
              className="input"
              style={{ width: "100%", fontFamily: "var(--mono)", fontSize: "0.85rem" }}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: "1.5rem", marginBottom: "1.5rem" }}>
            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Max Dollar Exposure (USD)
              </label>
              <input
                type="number"
                min="100"
                step="500"
                value={maxPositionUsd}
                onChange={(e) => setMaxPositionUsd(Number(e.target.value))}
                className="input"
                style={{ width: "100%" }}
              />
            </div>

            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Min Spread Threshold (BPS)
              </label>
              <input
                type="number"
                min="5"
                max="500"
                value={spreadThresholdBps}
                onChange={(e) => setSpreadThresholdBps(Number(e.target.value))}
                className="input"
                style={{ width: "100%" }}
              />
              <span style={{ fontSize: "0.75rem", color: "var(--muted)" }}>30 BPS = 0.30 cents</span>
            </div>

            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                CLOB Order Type
              </label>
              <select
                value={clobOrderType}
                onChange={(e) => setClobOrderType(e.target.value)}
                className="select"
                style={{ width: "100%" }}
              >
                <option value="GTC">GTC (Good &apos;Til Cancelled)</option>
                <option value="FAK">FAK (Fill and Kill / IOC)</option>
                <option value="FOK">FOK (Fill or Kill)</option>
              </select>
            </div>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary">
            {loading ? "Saving..." : "Save Polymarket Strategy"}
          </button>
        </form>
      </div>
    </AppShell>
  );
}
