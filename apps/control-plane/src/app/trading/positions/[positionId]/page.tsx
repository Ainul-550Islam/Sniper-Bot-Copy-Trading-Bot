"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { PositionDetail } from "@/components/trading/PositionDetail";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi } from "@/lib/api/trading-detail-api";
import type { TenantPosition } from "@/lib/customer-trading-api";

export default function PositionDetailPage() {
  const params = useParams();
  const positionId = typeof params?.positionId === "string" ? params.positionId : "";
  const [position, setPosition] = useState<TenantPosition | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!positionId) return;
    setLoading(true);
    setError(null);
    try {
      const next = await tradingDetailApi.position(positionId);
      // A body without an id is not a position record; surface it as a
      // retrieval failure rather than rendering an empty detail card.
      if (next && typeof next.id === "string") {
        setPosition(next);
      } else {
        setError(`The server returned no position record for ${positionId}.`);
      }
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`Position ${positionId} not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [positionId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Position Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Position: {positionId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Authoritative position row from the tenant trading data plane.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading position…</div>
      ) : position ? (
        <PositionDetail position={position} />
      ) : null}
    </AppShell>
  );
}
