"use client";

/**
 * Polymarket V3 order/fill/status page (PROMPT 5 §K, file 87).
 *
 * The caller's Polymarket mirror book + fills (`/api/tenant/polymarket/*`)
 * plus the module controls card.
 */

import { useCallback, useEffect, useState } from "react";
import ModuleCards from "@/components/trading/ModuleCards";
import { tenantRequest, classifyTradingError, isRetryable, TradingSurfaceState, tradingStateMessage } from "@/lib/customer-trading-api";

interface PolyOrderRow {
  id: string;
  market?: string | null;
  side?: string | null;
  status?: string | null;
  created_at?: string | null;
  [key: string]: unknown;
}

export default function PolymarketPage() {
  const [orders, setOrders] = useState<PolyOrderRow[] | null>(null);
  const [fills, setFills] = useState<unknown[] | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const [o, f] = await Promise.all([
        tenantRequest<{ items?: PolyOrderRow[] } | PolyOrderRow[]>("/api/tenant/polymarket/orders?limit=25"),
        tenantRequest<{ items?: unknown[] } | unknown[]>("/api/tenant/polymarket/fills?limit=25"),
      ]);
      const orderList = Array.isArray(o) ? o : o.items ?? [];
      const fillList = Array.isArray(f) ? f : f.items ?? [];
      setOrders(orderList);
      setFills(fillList);
      setState({ kind: orderList.length === 0 && fillList.length === 0 ? "empty" : "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main id="main" className="stack">
      <h1>Polymarket</h1>
      <section className="card" aria-label="Polymarket mirror book">
        <h2>Mirror orders (V3)</h2>
        {state.kind === "loading" && <p>Loading your Polymarket mirror book…</p>}
        {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
          <p role="alert" className="error">
            {tradingStateMessage(state)}
            {isRetryable(state) && (
              <>
                <br />
                <button onClick={() => void load()} className="link">Retry</button>
              </>
            )}
          </p>
        )}
        {state.kind === "empty" && <p>No Polymarket orders or fills yet. Mirrored V3 activity appears here.</p>}
        {state.kind === "ready" && orders && (
          <table>
            <thead>
              <tr><th>Order</th><th>Market</th><th>Side</th><th>Status</th><th>Created</th></tr>
            </thead>
            <tbody>
              {orders.map((o) => (
                <tr key={o.id}>
                  <td><code>{o.id.slice(0, 10)}…</code></td>
                  <td>{o.market ?? "—"}</td>
                  <td>{o.side ?? "—"}</td>
                  <td>{o.status ?? "—"}</td>
                  <td>{o.created_at ? new Date(o.created_at).toLocaleString() : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {state.kind === "ready" && fills && (
          <p className="muted">{fills.length} fill record(s) in the current window.</p>
        )}
      </section>
      <ModuleCards />
    </main>
  );
}
