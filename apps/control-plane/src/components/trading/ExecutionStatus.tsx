"use client";

/**
 * Per-order execution lifecycle status (PROMPT 6 §I).
 *
 * Given one order id, this component loads that order's execution
 * records from the caller's tenant data plane and classifies each row
 * into the honest lifecycle states the server can produce:
 *
 *   - `submitted` — non-terminal, the attempt is in flight
 *   - `confirmed` — non-terminal per the tenant read model; shown as
 *     the venue-acknowledged state
 *   - `failed`    — the row carries an error, or the status says so
 *   - anything else is rendered verbatim (never re-labelled)
 *
 * A summary line counts each class so a customer can see at a glance
 * whether an order is still in flight, acknowledged, or failing. The
 * custody-signature column shows whether the execution row carries a
 * signature (custody-signed) or not (unsigned/none) — it reports what
 * the server returned, it never infers a signature client-side.
 */

import { useCallback, useEffect, useState } from "react";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  type ExecutionsPage,
  type TenantExecution,
  type TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

type LifecycleClass = "in_flight" | "acknowledged" | "failed" | "other";

function classifyExecution(row: TenantExecution): LifecycleClass {
  if (row.error !== null && row.error !== "") return "failed";
  if (row.status === "failed") return "failed";
  if (row.status === "submitted") return "in_flight";
  if (row.status === "confirmed") return "acknowledged";
  return "other";
}

const CLASS_LABEL: Record<LifecycleClass, string> = {
  in_flight: "in flight",
  acknowledged: "acknowledged",
  failed: "failed",
  other: "other",
};

export default function ExecutionStatus({ orderId }: { orderId: string }) {
  const [page, setPage] = useState<ExecutionsPage | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.executionsForOrder(orderId);
      setPage(response);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, [orderId]);

  useEffect(() => {
    void load();
  }, [load]);

  const counts: Record<LifecycleClass, number> = {
    in_flight: 0,
    acknowledged: 0,
    failed: 0,
    other: 0,
  };
  if (page) {
    for (const row of page.items) counts[classifyExecution(row)] += 1;
  }

  return (
    <section className="card" aria-label={`Execution status for order ${orderId}`} data-order-id={orderId}>
      <h3>Execution status — order {orderId}</h3>
      {state.kind === "loading" && <p>Loading execution records…</p>}
      {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
        <p role="alert" className="error">
          {tradingStateMessage(state)}
          {isRetryable(state) && (
            <>
              <br />
              <button onClick={() => void load()} className="link">
                Retry
              </button>
            </>
          )}
        </p>
      )}
      {state.kind === "empty" && (
        <p>No execution records yet for this order — records appear as the order moves toward the venue.</p>
      )}
      {state.kind === "ready" && page && (
        <>
          <p className="muted">
            {page.items.length} execution record{page.items.length === 1 ? "" : "s"}:{" "}
            {counts.in_flight} {CLASS_LABEL.in_flight}, {counts.acknowledged} {CLASS_LABEL.acknowledged},{" "}
            {counts.failed} {CLASS_LABEL.failed}
            {counts.other > 0 ? `, ${counts.other} ${CLASS_LABEL.other}` : ""}.
          </p>
          <table>
            <thead>
              <tr>
                <th>At</th>
                <th>Kind</th>
                <th>Status</th>
                <th>Qty</th>
                <th>Price</th>
                <th>Custody signature</th>
                <th>Error</th>
              </tr>
            </thead>
            <tbody>
              {page.items.map((row) => (
                <tr key={row.id}>
                  <td>{new Date(row.at).toLocaleString()}</td>
                  <td>{row.kind}</td>
                  <td>
                    {row.status}
                    {classifyExecution(row) === "failed" ? " ⚠" : ""}
                  </td>
                  <td>{row.qty ?? "—"}</td>
                  <td>{row.price ?? "—"}</td>
                  <td>{row.signature ? <code>{row.signature.slice(0, 16)}…</code> : "none"}</td>
                  <td>{row.error ? <span title={row.error}>{row.error.slice(0, 60)}</span> : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      <p className="muted">
        Execution records are your organization&apos;s own data, resolved server-side from the
        authenticated credential; this component only renders them.
      </p>
    </section>
  );
}
