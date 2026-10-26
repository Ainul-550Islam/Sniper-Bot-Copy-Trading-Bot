"use client";

import type { DcaRun, DcaSchedule } from "@/lib/api/dca-api";

interface RunsPanelProps {
  schedule: DcaSchedule;
  runs: DcaRun[] | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
}

export function RunsPanel({ schedule, runs, loading, error, onClose }: RunsPanelProps) {
  return (
    <section className="card" aria-label={`Runs for DCA schedule ${schedule.id}`} style={{ borderLeft: "4px solid var(--accent)" }}>
      <div className="row-between">
        <div>
          <h2>Run history</h2>
          <p className="muted small">Schedule <code>{schedule.id}</code> · <code>{schedule.mint}</code></p>
        </div>
        <button type="button" className="small" onClick={onClose}>Close runs</button>
      </div>

      {loading && <p className="muted">Loading recorded runs…</p>}
      {error && <p role="alert" className="error">{error}</p>}
      {!loading && !error && runs?.length === 0 && (
        <p className="muted">No runs have been recorded for this schedule yet.</p>
      )}
      {!loading && !error && runs && runs.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Run</th>
              <th>Scheduled slot</th>
              <th>Amount</th>
              <th>Status</th>
              <th>Recorded</th>
            </tr>
          </thead>
          <tbody>
            {runs.map((run) => (
              <tr key={run.run_id}>
                <td>{run.run_id}</td>
                <td>{new Date(run.slot_at).toLocaleString()}</td>
                <td>{run.amount_sol} SOL</td>
                <td>{run.status}</td>
                <td>{new Date(run.created_at).toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export default RunsPanel;
