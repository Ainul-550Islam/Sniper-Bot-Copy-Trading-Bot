"use client";

/**
 * Tenant orders page (PROMPT 5 §K, file 82; drill-down via the
 * ExecutionStatus component since PROMPT 6 §I).
 *
 * The caller's own orders, server-paginated. The executions-per-order
 * drill-down is the shared `ExecutionStatus` component: it classifies
 * each execution row into the honest lifecycle states (in flight /
 * acknowledged / failed) and renders the record table for the selected
 * order.
 */

import { useState } from "react";
import OrderTable from "@/components/trading/OrderTable";
import ExecutionStatus from "@/components/trading/ExecutionStatus";

export default function OrdersPage() {
  const [drillOrderId, setDrillOrderId] = useState<string | null>(null);

  return (
    <main id="main" className="stack">
      <h1>Orders</h1>
      <OrderTable onShowExecutions={(id) => setDrillOrderId(id)} />
      {drillOrderId && (
        <>
          <ExecutionStatus orderId={drillOrderId} />
          <button className="link" onClick={() => setDrillOrderId(null)}>
            Close
          </button>
        </>
      )}
    </main>
  );
}
