"use client";

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";
import { units, whole } from "@/lib/strategy-form";

/** Creates a paper profile using the server's CopyStrategyParams. */
export default function CopyConfigPage() {
  const [name, setName] = useState("");
  const [leaders, setLeaders] = useState("");
  const [exposure, setExposure] = useState("");
  const [allocation, setAllocation] = useState("");
  const [slippage, setSlippage] = useState("");
  const [timeout, setTimeoutValue] = useState("");
  const [ratio, setRatio] = useState("");
  const [buys, setBuys] = useState(true);
  const [sells, setSells] = useState(false);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setLoading(true);
    setStatus(null);
    try {
      await createStrategy({ module_family: "copy", name, description: "Copy trading strategy profile", parameters: {
        max_exposure_usd_cents: units(exposure, 2, "Maximum exposure", true),
        allocation_per_trade_lamports: units(allocation, 9, "Trade allocation"),
        max_slippage_bps: whole(slippage, "Slippage", 0, 5000),
        mirror_buys: buys,
        mirror_sells: sells,
        stale_event_timeout_seconds: whole(timeout, "Stale event timeout", 0, 31536000),
        copy_ratio_pct: whole(ratio, "Copy ratio", 0, 1000),
        dry_run: true,
        // Stored as profile metadata; the read-only leaders endpoint is not mutated.
        lead_wallets: leaders.split("\n").map((s) => s.trim()).filter(Boolean),
      } });
      setStatus({ message: "Copy trading strategy saved to the authenticated tenant.", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save configuration", ok: false });
    } finally { setLoading(false); }
  }
  return (
    <AppShell title="Copy Trading Configuration">
      <h1>Copy Trading Engine Configuration</h1>
      <p className="muted">Save a paper profile. Leader addresses below are profile notes; saving does not add a tracked leader or activate the module.</p>
      {status && <p role="status" style={{ color: status.ok ? "var(--ok)" : "var(--bad)" }}>{status.message}</p>}
      <form onSubmit={save} className="card stack">
        <label>Strategy Profile Name<input required value={name} onChange={(e) => setName(e.target.value)} className="input" /></label>
        <label>Leader Wallet Addresses (profile notes, one per line)<textarea value={leaders} onChange={(e) => setLeaders(e.target.value)} rows={4} className="input" /></label>
        <label>Maximum Exposure (USD)<input required inputMode="decimal" value={exposure} onChange={(e) => setExposure(e.target.value)} className="input" /></label>
        <label>Allocation Per Trade (SOL)<input required inputMode="decimal" value={allocation} onChange={(e) => setAllocation(e.target.value)} className="input" /></label>
        <label>Maximum Slippage (BPS)<input required inputMode="numeric" value={slippage} onChange={(e) => setSlippage(e.target.value)} className="input" /></label>
        <label>Stale Event Timeout (seconds)<input required inputMode="numeric" value={timeout} onChange={(e) => setTimeoutValue(e.target.value)} className="input" /></label>
        <label>Copy Ratio (%)<input required inputMode="numeric" value={ratio} onChange={(e) => setRatio(e.target.value)} className="input" /></label>
        <label><input type="checkbox" checked={buys} onChange={(e) => setBuys(e.target.checked)} /> Mirror buys</label>
        <label><input type="checkbox" checked={sells} onChange={(e) => setSells(e.target.checked)} /> Mirror sells</label>
        <button type="submit" disabled={loading} className="btn btn-primary">{loading ? "Saving Configuration..." : "Save Copy Trading Configuration"}</button>
      </form>
    </AppShell>
  );
}
