"use client";

import { useEffect, useState, useCallback } from "react";
import { KillSwitchPanel } from "@/components/operator/KillSwitchPanel";
import { operator } from "@/lib/api";

export default function OperatorKillSwitchPage() {
  const [killSwitchActive, setKillSwitchActive] = useState<boolean>(false);
  const [loading, setLoading] = useState(true);

  const loadStatus = useCallback(async () => {
    try {
      const summary = (await operator.status()) as {
        kill_switch?: boolean;
      };
      if (summary?.kill_switch !== undefined) {
        setKillSwitchActive(Boolean(summary.kill_switch));
      }
    } catch {
      // Keep default on unattached backend
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

      <KillSwitchPanel
        killSwitchActive={killSwitchActive}
        onRefresh={loadStatus}
      />
    </article>
  );
}
