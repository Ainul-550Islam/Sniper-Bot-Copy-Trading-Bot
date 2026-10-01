"use client";

/**
 * Execution lifecycle page (PROMPT 5 §K, file 84).
 *
 * Browses the caller's execution records over a time window (default:
 * the last 24 hours, computed locally only as window bounds — every row
 * is server data).
 */

import { useCallback, useEffect, useState } from "react";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  tradingStateMessage,
  type ExecutionsPage,
  type TradingSurfaceState,
} from "@/lib/customer-trading-api";

export default function ExecutionsPage() {
  const [since, setSince] = useState<string>(() => new Date(Date.now() - 24 * 3600 * 1000).toISOString());
  const [until, setUntil] = useState<string>(() => new Date().toISOString());
  const [page, setPage] = useState<ExecutionsPage | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.executionsWindow(since, until);
      setPage(response);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, [since, until]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main id="main" className="stack">
      <h1>Executions</h1>
      <section className="card">
        <h2>Window (UTC)</h2>
        <div className="row">
          <label>
            Since{" "}
            <input type="datetime-local" value={since.slice(0, 16)} onChange={(e) => setSince(new Date(e.target.value).toISOString())} />
          </label>
          <label>
            Until{" "}
            <input type="datetime-local" value={until.slice(0, 16)} onChange={(e) => setUntil(new Date(e.target.value).toISOString())} />
          </label>
          <button onClick={() => void load()}>Load</button>
        </div>
      </section>
      <section className="card" aria-label="Execution rows">
        {state.kind === "loading" && <p>Loading executions…</p>}
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
        {state.kind === "empty" && <p>No executions recorded in this window.</p>}
        {state.kind === "ready" && page && (
          <table>
            <thead>
              <tr>
                <th>At</th>
                <th>Order</th>
                <th>Kind</th>
                <th>Status</th>
                <th>Qty</th>
                <th>Price</th>
                <th>Signature</th>
                <th>Error</th>
              </tr>
            </thead>
            <tbody>
              {page.items.map((x) => (
                <tr key={x.id}>
                  <td>{new Date(x.at).toLocaleString()}</td>
                  <td><code>{x.order_id.slice(0, 8)}…</code></td>
                  <td>{x.kind}</td>
                  <td>{x.status}</td>
                  <td>{x.qty ?? "—"}</td>
                  <td>{x.price ?? "—"}</td>
                  <td>{x.signature ? <code>{x.signature.slice(0, 10)}…</code> : "—"}</td>
                  <td>{x.error ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </main>
  );
}
