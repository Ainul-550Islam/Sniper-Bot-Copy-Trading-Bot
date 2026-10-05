"use client";

/**
 * Tenant Orders Management Page (PROMPT 5 §K, file 82 & Commercial Readiness).
 *
 * Full commercial order table with server pagination, drill-down execution trail,
 * cancellation confirmation dialogs, and exact numeric formatting.
 */

import { useState } from "react";
import { AppShell } from "@/components/AppShell";
import OrderTable from "@/components/trading/OrderTable";
import ExecutionStatus from "@/components/trading/ExecutionStatus";

export default function OrdersPage() {
  const [drillOrderId, setDrillOrderId] = useState<string | null>(null);

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Orders Book &amp; Lifecycle</h1>
            <p className="muted">
              Inspect submitted, filled, and pending algorithmic orders with sub-second execution drill-down.
            </p>
          </div>
        </div>

        <OrderTable onShowExecutions={(id) => setDrillOrderId(id)} />

        {drillOrderId && (
          <div className="card stack" style={{ borderLeft: "4px solid var(--accent)" }}>
            <div className="row-between">
              <h2>Order Execution Trace: {drillOrderId}</h2>
              <button className="small" onClick={() => setDrillOrderId(null)}>
                ✕ Close Trace
              </button>
            </div>
            <ExecutionStatus orderId={drillOrderId} />
          </div>
        )}
      </div>
    </AppShell>
  );
}
