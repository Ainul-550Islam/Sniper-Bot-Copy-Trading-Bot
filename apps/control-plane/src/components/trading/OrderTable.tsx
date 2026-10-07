"use client";

/**
 * Tenant order table with SERVER pagination (PROMPT 5 §K, file 90).
 *
 * Pagination is keyset-based: the server returns `next_cursor` and this
 * component simply follows it — the page state lives on the server, the
 * browser never re-implements filtering. Orders are the caller's OWN
 * orders only (the scope is resolved from the authenticated credential
 * server-side).
 */

import { useCallback, useEffect, useState } from "react";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  OrdersPage,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export default function OrderTable({ onShowExecutions }: { onShowExecutions?: (orderId: string) => void }) {
  const [page, setPage] = useState<OrdersPage | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async (nextCursor: string | null) => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.orders(nextCursor);
      setPage(response);
      setCursor(nextCursor);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load(null));
  }, [load]);

  async function cancel(id: string) {
    try {
      await customerTrading.cancelOrder(id);
      await load(cursor);
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }

  return (
    <section className="card" aria-label="Orders">
      <h2>Orders</h2>
      {state.kind === "loading" && <p>Loading orders…</p>}
      {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
        <p role="alert" className="error">
          {tradingStateMessage(state)}
          {isRetryable(state) && (
            <>
              <br />
              <button onClick={() => void load(cursor)} className="link">
                Retry
              </button>
            </>
          )}
        </p>
      )}
      {state.kind === "empty" && <p>No orders yet — your organization&apos;s first order will appear here.</p>}
      {state.kind === "ready" && page && (
        <>
          <table>
            <thead>
              <tr>
                <th>Time</th>
                <th>Module</th>
                <th>Side</th>
                <th>Symbol</th>
                <th>Venue</th>
                <th>Qty</th>
                <th>Price</th>
                <th>Status</th>
                <th>Mode</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {page.items.map((o) => (
                <tr key={o.id}>
                  <td>{new Date(o.created_at).toLocaleString()}</td>
                  <td>{o.module}</td>
                  <td>{o.side}</td>
                  <td>{o.symbol}</td>
                  <td>{o.venue}</td>
                  <td>{o.qty}</td>
                  <td>{o.price ?? "—"}</td>
                  <td>{o.error ? <span title={o.error}>{o.status} ⚠</span> : o.status}</td>
                  <td>{o.mode}</td>
                  <td>
                    {onShowExecutions && (
                      <button className="link" onClick={() => onShowExecutions(o.id)}>
                        executions
                      </button>
                    )}
                    {o.status !== "filled" && o.status !== "cancelled" && (
                      <button className="link" onClick={() => void cancel(o.id)}>
                        cancel
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <nav className="row">
            <button disabled={cursor === null} onClick={() => void load(null)}>
              ⟲ First page
            </button>
            <button disabled={page.next_cursor === null} onClick={() => void load(page.next_cursor)}>
              Older orders →
            </button>
          </nav>
        </>
      )}
    </section>
  );
}
