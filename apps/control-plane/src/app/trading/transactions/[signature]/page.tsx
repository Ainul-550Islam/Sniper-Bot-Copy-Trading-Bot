"use client";

import { useCallback, useEffect, useRef, useState } from "react";
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
  const requestVersion = useRef(0);

  const load = useCallback(async () => {
    const requestId = ++requestVersion.current;
    if (!signature) {
      setTransaction(null);
      setError("Transaction signature is missing.");
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    setTransaction(null);
    try {
      const next = await tradingDetailApi.transaction(signature);
      if (requestId !== requestVersion.current) return;
      // A body without a signature is not a transaction record; surface honestly.
      if (next && typeof next.signature === "string" && next.signature.length > 0) {
        setTransaction(next);
      } else {
        setError(`The server returned no transaction record for ${signature}.`);
      }
    } catch (err: unknown) {
      if (requestId !== requestVersion.current) return;
      setTransaction(null);
      if (err instanceof ApiError && err.status === 404) {
        setError(`Transaction ${signature.slice(0, 14)}… not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, [signature]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
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
