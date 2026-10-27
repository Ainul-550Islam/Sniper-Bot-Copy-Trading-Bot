"use client";

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { RecoveryPanel } from "@/components/trading/RecoveryPanel";
import { toDisplayError } from "@/lib/api";
import {
  tradingDetailApi,
  type RecoveryIntentsResponse,
  type RecoverySweepResponse,
} from "@/lib/api/trading-detail-api";

export default function RecoveryPage() {
  const [intents, setIntents] = useState<RecoveryIntentsResponse["items"]>([]);
  const [sweeps, setSweeps] = useState<RecoverySweepResponse["items"]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [intentsResponse, sweepsResponse] = await Promise.all([
        tradingDetailApi.recoveryIntents(),
        tradingDetailApi.recoverySweep(),
      ]);
      setIntents(Array.isArray(intentsResponse.items) ? intentsResponse.items : []);
      setSweeps(Array.isArray(sweepsResponse.items) ? sweepsResponse.items : []);
    } catch (err: unknown) {
      setIntents([]);
      setSweeps([]);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Recovery">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Recovery</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Custody recovery intents and sweeps recorded by the recovery data
          plane for this organization.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading recovery records…</div>
      ) : (
        <RecoveryPanel intents={intents} sweeps={sweeps} />
      )}
    </AppShell>
  );
}
