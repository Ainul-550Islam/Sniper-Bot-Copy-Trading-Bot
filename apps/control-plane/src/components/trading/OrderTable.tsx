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

import Link from "next/link";
import { useCallback, useEffect, useRef, useState } from "react";
import { sessionStore } from "@/lib/auth";
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
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setPage(null);
    setCursor(null);
    setState({ kind: "loading" });
  }), []);

  const load = useCallback(async (nextCursor: string | null) => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setPage(null);
    setState({ kind: "loading" });
    if (!selectedOrganizationId) {
      setCursor(null);
      setState({ kind: "empty" });
      return;
    }
    try {
      const response = await customerTrading.orders(nextCursor);
      if (requestId !== requestVersion.current) return;
      if (response.organization_id !== selectedOrganizationId) {
        throw new Error("Order response does not match the selected organization");
      }
      setPage(response);
      setCursor(nextCursor);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      if (requestId === requestVersion.current) {
        setPage(null);
        setState(classifyTradingError(e));
      }
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load(null);
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

  async function cancel(id: string) {
    const actionOrganizationId = selectedOrganizationId;
    if (!actionOrganizationId || actionOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    try {
      await customerTrading.cancelOrder(id);
      if (actionOrganizationId === sessionStore.getSnapshot().selectedOrganizationId) {
        await load(cursor);
      }
    } catch (e) {
      if (actionOrganizationId === sessionStore.getSnapshot().selectedOrganizationId) {
        setState(classifyTradingError(e));
      }
    }
  }

  return (
    <section className="card" aria-label="Orders">
      <h2>Orders</h2>
      {!selectedOrganizationId && <p>Select an organization to view orders.</p>}
      {selectedOrganizationId && state.kind === "loading" && <p>Loading orders…</p>}
      {selectedOrganizationId && state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
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
      {selectedOrganizationId && state.kind === "empty" && <p>No orders yet — your organization&apos;s first order will appear here.</p>}
      {selectedOrganizationId && state.kind === "ready" && page?.organization_id === selectedOrganizationId && page && (
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
                <th>Details</th>
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
                    <Link href={`/trading/orders/${o.id}`}>Details</Link>
                  </td>
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
