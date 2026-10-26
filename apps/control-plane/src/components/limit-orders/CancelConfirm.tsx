"use client";

import type { LimitOrder } from "@/lib/api/limit-orders-api";

interface CancelConfirmProps {
  order: LimitOrder;
  busy: boolean;
  onConfirm: () => void;
  onDismiss: () => void;
}

/** Confirmation step for cancelling one server-returned limit order. */
export function CancelConfirm({ order, busy, onConfirm, onDismiss }: CancelConfirmProps) {
  return (
    <div className="card" role="alertdialog" aria-label="Confirm limit order cancellation" style={{ borderLeft: "4px solid var(--bad)" }}>
      <h3>Cancel this limit order?</h3>
      <p className="muted">
        Order <code>{order.id}</code> ({order.side} {order.amount} at {order.price_sol} SOL)
        will be sent to the server for cancellation. The list will be refetched and will show
        only the status the server returns.
      </p>
      <div className="row">
        <button type="button" onClick={onDismiss} disabled={busy}>Keep order</button>
        <button type="button" className="danger" onClick={onConfirm} disabled={busy}>
          {busy ? "Cancelling…" : "Confirm cancellation"}
        </button>
      </div>
    </div>
  );
}

export default CancelConfirm;
