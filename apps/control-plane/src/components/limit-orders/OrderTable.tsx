"use client";

import type { LimitOrder } from "@/lib/api/limit-orders-api";

interface OrderTableProps {
  orders: LimitOrder[];
  canManage: boolean;
  busyOrderId: string | null;
  onRequestCancel: (order: LimitOrder) => void;
}

export function OrderTable({ orders, canManage, busyOrderId, onRequestCancel }: OrderTableProps) {
  if (orders.length === 0) {
    return <p className="muted">No limit orders recorded for this organization yet.</p>;
  }

  return (
    <div style={{ overflowX: "auto" }}>
      <table>
        <thead>
          <tr>
            <th>Created</th>
            <th>Mint</th>
            <th>Side</th>
            <th>Trigger</th>
            <th>Price</th>
            <th>Amount</th>
            <th>Status</th>
            <th>Expires</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {orders.map((order) => (
            <tr key={order.id}>
              <td>{new Date(order.created_at).toLocaleString()}</td>
              <td><code>{order.mint}</code></td>
              <td>{order.side}</td>
              <td>{order.trigger}</td>
              <td>{order.price_sol} SOL</td>
              <td>{order.amount}</td>
              <td>
                <span className="tag">{order.status}</span>
                {order.last_error && <span title={order.last_error}> ⚠</span>}
              </td>
              <td>{order.expires_at ? new Date(order.expires_at).toLocaleString() : "Good til cancelled"}</td>
              <td>
                {order.status === "active" && (
                  <button
                    type="button"
                    className="link"
                    disabled={!canManage || busyOrderId === order.id}
                    onClick={() => onRequestCancel(order)}
                  >
                    Cancel
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default OrderTable;
