"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { RecoveryPanel } from "@/components/trading/RecoveryPanel";
import { toDisplayError } from "@/lib/api";
import { sessionStore } from "@/lib/auth";
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
  const [sessionVersion, setSessionVersion] = useState(0);
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setIntents([]);
    setSweeps([]);
    setLoading(true);
    setError(null);
  }), []);

  const load = useCallback(async () => {
    const requestId = ++requestVersion.current;
    setIntents([]);
    setSweeps([]);
    setLoading(true);
    setError(null);
    try {
      const [intentsResponse, sweepsResponse] = await Promise.all([
        tradingDetailApi.recoveryIntents(),
        tradingDetailApi.recoverySweep(),
      ]);
      if (requestId !== requestVersion.current) return;
      setIntents(Array.isArray(intentsResponse.items) ? intentsResponse.items : []);
      setSweeps(Array.isArray(sweepsResponse.items) ? sweepsResponse.items : []);
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setIntents([]);
        setSweeps([]);
        setError(toDisplayError(err));
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

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
