"use client";

import { useEffect, useState, useCallback } from "react";
import { KillSwitchPanel } from "@/components/operator/KillSwitchPanel";
import { getStatus } from "@/lib/api/operator-api";

export default function OperatorKillSwitchPage() {
  const [killSwitchActive, setKillSwitchActive] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      const summary = await getStatus();
      if (typeof summary.kill_switch !== "boolean") throw new Error("Invalid kill switch status");
      setKillSwitchActive(summary.kill_switch);
      setError(null);
    } catch (err) {
      setKillSwitchActive(null);
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
          <h1>Emergency Stop &amp; Safeguards (/api/kill)</h1>
          <p className="muted">
            Immediate trading suspension across all venues and strategies.
            Global emergency stops require typed confirmation and are recorded in the immutable audit trail.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadStatus()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && <p role="alert">{error}</p>}
      {!loading && !error && killSwitchActive !== null && <KillSwitchPanel
        killSwitchActive={killSwitchActive}
        onRefresh={loadStatus}
      />}
    </article>
  );
}
