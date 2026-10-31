"use client";

import { useEffect, useState, useCallback } from "react";
import { ModeSwitcher } from "@/components/operator/ModeSwitcher";
import { getStatus } from "@/lib/api/operator-api";

export default function OperatorModePage() {
  const [currentMode, setCurrentMode] = useState<string | null>(null);
  const [liveAllowed, setLiveAllowed] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      const summary = await getStatus();
      if (typeof summary.execution_mode !== "string" || !summary.execution_mode || typeof summary.live_allowed !== "boolean") throw new Error("Invalid execution mode status");
      setCurrentMode(summary.execution_mode);
      setLiveAllowed(summary.live_allowed);
      setError(null);
    } catch (err) {
      setCurrentMode(null);
      setLiveAllowed(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadStatus());
  }, [loadStatus]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Execution Mode Control (/api/mode)</h1>
          <p className="muted">
            Execution mode governs the transaction pipeline across all connected DEXs and venues.
            Switching to live trading sends real signed transactions and requires typed confirmation.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadStatus()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && <p role="alert">{error}</p>}
      {!loading && !error && currentMode !== null && liveAllowed !== null && <ModeSwitcher
        currentMode={currentMode}
        liveAllowed={liveAllowed}
        onModeChanged={(res) => {
          setCurrentMode(res.mode);
          setLiveAllowed(res.live_allowed);
        }}
      />}
    </article>
  );
}
