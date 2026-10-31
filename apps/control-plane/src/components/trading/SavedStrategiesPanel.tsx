"use client";

import { useEffect, useState } from "react";
import { listStrategies, type StrategyModule, type StrategyRecord } from "@/lib/api/strategy-api";

/** Saved profiles are tenant records, not proof that an engine is running. */
export default function SavedStrategiesPanel({ module }: { module: StrategyModule }) {
  const [records, setRecords] = useState<StrategyRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    listStrategies(module).then((items) => { if (alive) { setRecords(items); setError(null); } })
      .catch((err: unknown) => { if (alive) setError(err instanceof Error ? err.message : "Unable to load strategies"); });
    return () => { alive = false; };
  }, [module]);
  return <section className="card">
    <h2>Saved strategy profiles</h2>
    <p className="muted">Profiles are stored for this tenant; they do not by themselves change the active module configuration.</p>
    {error ? <p role="alert">{error}</p> : records === null ? <p>Loading profiles…</p> : records.length === 0 ? <p>No saved profiles.</p> :
      <ul>{records.map((record) => <li key={record.id}><strong>{record.name}</strong> — {record.status} (v{record.version})<pre>{JSON.stringify(record.parameters, null, 2)}</pre></li>)}</ul>}
  </section>;
}
