"use client";

import { useEffect, useState, useCallback } from "react";
import { ModuleToggleTable } from "@/components/operator/ModuleToggleTable";
import { getModules, type ModuleStatus } from "@/lib/api/operator-api";

export default function OperatorModulesPage() {
  const [modules, setModules] = useState<ModuleStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadModules = useCallback(async () => {
    try {
      const data = await getModules();
      setModules(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadModules());
  }, [loadModules]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Bot Modules &amp; Supervisor (/api/modules)</h1>
          <p className="muted">
            Manage bot runtimes across Sniper, Copy Trading, Polymarket, and Telegram.
            Disabling or enabling any module is an audited operator action requiring typed confirmation.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadModules()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      <ModuleToggleTable modules={modules} onRefresh={loadModules} />
    </article>
  );
}
