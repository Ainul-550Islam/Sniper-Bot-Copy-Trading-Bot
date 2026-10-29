"use client";

import { useEffect, useState, useCallback } from "react";
import { ModeSwitcher } from "@/components/operator/ModeSwitcher";
import { operator } from "@/lib/api";

export default function OperatorModePage() {
  const [currentMode, setCurrentMode] = useState<string>("paper");
  const [liveAllowed, setLiveAllowed] = useState<boolean>(false);
  const [loading, setLoading] = useState(true);

  const loadStatus = useCallback(async () => {
    try {
      const summary = (await operator.status()) as {
        mode?: string;
        live_allowed?: boolean;
      };
      if (summary?.mode) setCurrentMode(summary.mode);
      if (summary?.live_allowed !== undefined) setLiveAllowed(summary.live_allowed);
    } catch {
      // In loopback / dev or minimal mock environments, keep default
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

      <ModeSwitcher
        currentMode={currentMode}
        liveAllowed={liveAllowed}
        onModeChanged={(res) => {
          setCurrentMode(res.mode);
          setLiveAllowed(res.live_allowed);
        }}
      />
    </article>
  );
}
