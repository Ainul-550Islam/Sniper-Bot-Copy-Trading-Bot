"use client";

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";
import { units, whole } from "@/lib/strategy-form";

/** Creates a paper strategy profile using the server's SniperStrategyParams. */
export default function SniperConfigPage() {
  const [name, setName] = useState("");
  const [entry, setEntry] = useState("");
  const [liquidity, setLiquidity] = useState("");
  const [slippage, setSlippage] = useState("");
  const [profit, setProfit] = useState("");
  const [stop, setStop] = useState("");
  const [trailing, setTrailing] = useState("");
  const [timeout, setTimeoutValue] = useState("");
  const [priority, setPriority] = useState("");
  const [antiMev, setAntiMev] = useState(true);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);

  async function save(event: React.FormEvent) {
    event.preventDefault();
    setLoading(true);
    setStatus(null);
    try {
      await createStrategy({
        module_family: "sniper", name, description: "Sniper strategy profile",
        parameters: {
          min_liquidity_lamports: units(liquidity, 9, "Minimum liquidity", true),
          max_slippage_bps: whole(slippage, "Slippage", 0, 5000),
          anti_mev_protection: antiMev,
          priority_fee_lamports: whole(priority, "Priority fee", 0, Number.MAX_SAFE_INTEGER),
          entry_amount_lamports: units(entry, 9, "Entry amount"),
          take_profit_pct: whole(profit, "Take profit", 0, 100000),
          stop_loss_pct: whole(stop, "Stop loss", 0, 100),
          trailing_stop_pct: whole(trailing, "Trailing stop", 0, 100),
          auto_sell_timeout_seconds: whole(timeout, "Auto-sell timeout", 0, 31536000),
          dry_run: true,
        },
      });
      setStatus({ message: "Sniper strategy saved to the authenticated tenant.", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save sniper configuration", ok: false });
    } finally { setLoading(false); }
  }
  return (
    <AppShell title="Sniper Configuration">
      <h1>Solana Sniper Strategy Configuration</h1>
      <p className="muted">Save a paper strategy profile. This does not by itself activate the module or change its separate runtime configuration.</p>
      {status && <p role="status" style={{ color: status.ok ? "var(--ok)" : "var(--bad)" }}>{status.message}</p>}
      <form onSubmit={save} className="card stack">
        <label>Strategy Profile Name<input required value={name} onChange={(e) => setName(e.target.value)} className="input" /></label>
        <label>Entry Amount (SOL)<input required inputMode="decimal" value={entry} onChange={(e) => setEntry(e.target.value)} className="input" /></label>
        <label>Minimum Liquidity (SOL)<input required inputMode="decimal" value={liquidity} onChange={(e) => setLiquidity(e.target.value)} className="input" /></label>
        <label>Maximum Slippage (BPS)<input required inputMode="numeric" value={slippage} onChange={(e) => setSlippage(e.target.value)} className="input" /></label>
        <label>Take Profit (%)<input required inputMode="numeric" value={profit} onChange={(e) => setProfit(e.target.value)} className="input" /></label>
        <label>Stop Loss (%)<input required inputMode="numeric" value={stop} onChange={(e) => setStop(e.target.value)} className="input" /></label>
        <label>Trailing Stop (%)<input required inputMode="numeric" value={trailing} onChange={(e) => setTrailing(e.target.value)} className="input" /></label>
        <label>Auto-sell Timeout (seconds)<input required inputMode="numeric" value={timeout} onChange={(e) => setTimeoutValue(e.target.value)} className="input" /></label>
        <label>Priority Fee (lamports)<input required inputMode="numeric" value={priority} onChange={(e) => setPriority(e.target.value)} className="input" /></label>
        <label><input type="checkbox" checked={antiMev} onChange={(e) => setAntiMev(e.target.checked)} /> Anti-MEV protection</label>
        <button type="submit" disabled={loading} className="btn btn-primary">{loading ? "Saving Configuration..." : "Save Sniper Configuration"}</button>
      </form>
    </AppShell>
  );
}
