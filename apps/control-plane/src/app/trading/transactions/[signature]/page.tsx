"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { TransactionDetail as TransactionDetailComponent } from "@/components/trading/TransactionDetail";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi, type TransactionDetail } from "@/lib/api/trading-detail-api";

export default function TransactionDetailPage() {
  const params = useParams();
  const signature = typeof params?.signature === "string" ? params.signature : "";
  const [transaction, setTransaction] = useState<TransactionDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!signature) return;
    setLoading(true);
    setError(null);
    try {
      const next = await tradingDetailApi.transaction(signature);
      // A body without a signature is not a transaction record; surface honestly.
      if (next && typeof next.signature === "string") {
        setTransaction(next);
      } else {
        setError(`The server returned no transaction record for ${signature}.`);
      }
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`Transaction ${signature.slice(0, 14)}… not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [signature]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Transaction Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Transaction: {signature.slice(0, 14)}…</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Public on-chain transaction data, shown for display only. The
          signature is never a credential and is never sent back from this page.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading transaction…</div>
      ) : transaction ? (
        <TransactionDetailComponent transaction={transaction} />
      ) : null}
    </AppShell>
  );
}
