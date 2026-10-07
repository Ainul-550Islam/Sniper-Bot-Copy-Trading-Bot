"use client";

/**
 * Execution Journal & Sub-Second Latency Trace (PROMPT 5 §K, file 84 & Commercial Readiness).
 *
 * Browses the tenant's execution records over a time window with Solana Explorer signature links,
 * failure categorization, venue identifiers, and recovery context.
 */

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { AppShell } from "@/components/AppShell";
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
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Execution Journal &amp; Audit Trail</h1>
            <p className="muted">
              Sub-second transaction signatures, order linkage, fill timestamps, and venue confirmations.
            </p>
          </div>
        </div>

        {/* Time Window Filter */}
        <section className="card">
          <h2>Filter Window (UTC)</h2>
          <div className="form-row" style={{ marginTop: "1rem" }}>
            <div className="form-group">
              <label htmlFor="since">Since (UTC)</label>
              <input
                id="since"
                type="datetime-local"
                value={since.slice(0, 16)}
                onChange={(e) => setSince(new Date(e.target.value).toISOString())}
              />
            </div>
            <div className="form-group">
              <label htmlFor="until">Until (UTC)</label>
              <input
                id="until"
                type="datetime-local"
                value={until.slice(0, 16)}
                onChange={(e) => setUntil(new Date(e.target.value).toISOString())}
              />
            </div>
            <div className="form-group" style={{ justifyContent: "flex-end" }}>
              <button className="primary" onClick={() => void load()}>
                Query Executions
              </button>
            </div>
          </div>
        </section>

        {/* Executions Table */}
        <section className="card" aria-label="Execution rows">
          <h2>Execution Records</h2>
          {state.kind === "loading" && <p>Loading executions from ledger…</p>}
          {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
            <div className="notice warn" role="alert">
              <p>{tradingStateMessage(state)}</p>
              {isRetryable(state) && (
                <button onClick={() => void load()} className="link" style={{ marginTop: "0.4rem" }}>
                  Retry
                </button>
              )}
            </div>
          )}
          {state.kind === "empty" && <p className="muted">No executions recorded in this window.</p>}
          {state.kind === "ready" && page && (
            <div style={{ marginTop: "1rem", overflowX: "auto" }}>
              <table>
                <thead>
                  <tr>
                    <th>Timestamp</th>
                    <th>Order Link</th>
                    <th>Kind</th>
                    <th>Status</th>
                    <th>Qty</th>
                    <th>Price</th>
                    <th>Solana Tx Signature</th>
                    <th>Error Detail</th>
                  </tr>
                </thead>
                <tbody>
                  {page.items.map((x) => (
                    <tr key={x.id}>
                      <td>{new Date(x.at).toLocaleString()}</td>
                      <td>
                        <code>{x.order_id.slice(0, 8)}…</code>
                      </td>
                      <td>
                        <span className="badge">{x.kind}</span>
                      </td>
                      <td>
                        <span className={`tag tag--${x.status === "confirmed" ? "healthy" : x.status === "failed" ? "error" : "running"}`}>
                          {x.status}
                        </span>
                      </td>
                      <td>{x.qty != null ? x.qty.toFixed(4) : "—"}</td>
                      <td>{x.price != null ? `$${x.price.toFixed(4)}` : "—"}</td>
                      <td>
                        {x.signature ? (
                          <Link
                            href={`https://solscan.io/tx/${x.signature}`}
                            target="_blank"
                            rel="noreferrer"
                          >
                            <code>{x.signature.slice(0, 10)}… ↗</code>
                          </Link>
                        ) : (
                          <span className="muted">—</span>
                        )}
                      </td>
                      <td>{x.error ? <span className="error small" title={x.error}>{x.error.slice(0, 40)}…</span> : "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      </div>
    </AppShell>
  );
}
