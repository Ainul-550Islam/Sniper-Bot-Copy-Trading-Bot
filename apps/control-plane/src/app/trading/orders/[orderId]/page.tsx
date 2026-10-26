"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { OrderDetail } from "@/components/trading/OrderDetail";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi } from "@/lib/api/trading-detail-api";
import type { TenantOrder } from "@/lib/customer-trading-api";

export default function OrderDetailPage() {
  const params = useParams();
  const orderId = typeof params?.orderId === "string" ? params.orderId : "";
  const [order, setOrder] = useState<TenantOrder | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!orderId) return;
    setLoading(true);
    setError(null);
    try {
      const next = await tradingDetailApi.order(orderId);
      // A body without an id is not an order record; surface it honestly.
      if (next && typeof next.id === "string") {
        setOrder(next);
      } else {
        setError(`The server returned no order record for ${orderId}.`);
      }
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`Order ${orderId} not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [orderId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Order Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Order: {orderId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Authoritative order row from the tenant trading data plane.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading order…</div>
      ) : order ? (
        <OrderDetail order={order} />
      ) : null}
    </AppShell>
  );
}
