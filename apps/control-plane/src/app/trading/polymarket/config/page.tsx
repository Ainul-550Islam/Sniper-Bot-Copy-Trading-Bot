"use client";

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";
import { units, whole } from "@/lib/strategy-form";

/** Creates a paper profile using the server's PolymarketStrategyParams. */
export default function PolymarketConfigPage() {
  const [name, setName] = useState("");
  const [conditions, setConditions] = useState("");
  const [position, setPosition] = useState("");
  const [exposure, setExposure] = useState("");
  const [spread, setSpread] = useState("");
  const [reprice, setReprice] = useState("");
  const [orderType, setOrderType] = useState("limit");
  const [cancel, setCancel] = useState(true);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setLoading(true);
    setStatus(null);
    try {
      await createStrategy({ module_family: "polymarket", name, description: "Polymarket strategy profile", parameters: {
        active_condition_ids: conditions.split("\n").map((s) => s.trim()).filter(Boolean),
        max_position_size_usdc_units: units(position, 6, "Maximum position"),
        max_market_exposure_usdc_units: units(exposure, 6, "Maximum market exposure", true),
        spread_threshold_bps: whole(spread, "Spread threshold", 0, 10000),
        reprice_interval_seconds: whole(reprice, "Reprice interval", 0, 31536000),
        cancel_stale_orders: cancel,
        dry_run: true,
        order_type: orderType,
      } });
      setStatus({ message: "Polymarket strategy saved to the authenticated tenant.", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save configuration", ok: false });
    } finally { setLoading(false); }
  }
  return (
    <AppShell title="Polymarket Configuration">
      <h1>Polymarket CLOB Strategy Configuration</h1>
      <p className="muted">Save a paper strategy profile; activation and module runtime configuration are separate operations.</p>
      {status && <p role="status" style={{ color: status.ok ? "var(--ok)" : "var(--bad)" }}>{status.message}</p>}
      <form onSubmit={save} className="card stack">
        <label>Strategy Profile Name<input required value={name} onChange={(e) => setName(e.target.value)} className="input" /></label>
        <label>Market Condition IDs (one per line)<textarea value={conditions} onChange={(e) => setConditions(e.target.value)} rows={3} className="input" /></label>
        <label>Maximum Position (USDC)<input required inputMode="decimal" value={position} onChange={(e) => setPosition(e.target.value)} className="input" /></label>
        <label>Maximum Market Exposure (USDC)<input required inputMode="decimal" value={exposure} onChange={(e) => setExposure(e.target.value)} className="input" /></label>
        <label>Spread Threshold (BPS)<input required inputMode="numeric" value={spread} onChange={(e) => setSpread(e.target.value)} className="input" /></label>
        <label>Reprice Interval (seconds)<input required inputMode="numeric" value={reprice} onChange={(e) => setReprice(e.target.value)} className="input" /></label>
        <label>Order Type<select value={orderType} onChange={(e) => setOrderType(e.target.value)} className="select"><option value="limit">Limit</option><option value="gtc">GTC</option><option value="fok">FOK</option></select></label>
        <label><input type="checkbox" checked={cancel} onChange={(e) => setCancel(e.target.checked)} /> Cancel stale orders</label>
        <button type="submit" disabled={loading} className="btn btn-primary">{loading ? "Saving..." : "Save Polymarket Strategy"}</button>
      </form>
    </AppShell>
  );
}
