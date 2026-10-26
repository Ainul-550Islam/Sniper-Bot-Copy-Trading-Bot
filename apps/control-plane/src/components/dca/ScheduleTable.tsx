"use client";

import type { DcaSchedule } from "@/lib/api/dca-api";

interface ScheduleTableProps {
  schedules: DcaSchedule[];
  canManage: boolean;
  busyScheduleId: string | null;
  onPause: (schedule: DcaSchedule) => void;
  onResume: (schedule: DcaSchedule) => void;
  onRequestCancel: (schedule: DcaSchedule) => void;
  onShowRuns: (schedule: DcaSchedule) => void;
}

export function ScheduleTable({
  schedules,
  canManage,
  busyScheduleId,
  onPause,
  onResume,
  onRequestCancel,
  onShowRuns,
}: ScheduleTableProps) {
  if (schedules.length === 0) {
    return <p className="muted">No DCA schedules recorded for this organization yet.</p>;
  }

  return (
    <div style={{ overflowX: "auto" }}>
      <table>
        <thead>
          <tr>
            <th>Created</th>
            <th>Mint</th>
            <th>Per run</th>
            <th>Interval</th>
            <th>Spent / budget</th>
            <th>Status</th>
            <th>Next run</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {schedules.map((schedule) => {
            const busy = busyScheduleId === schedule.id;
            return (
              <tr key={schedule.id}>
                <td>{new Date(schedule.created_at).toLocaleString()}</td>
                <td><code>{schedule.mint}</code></td>
                <td>{schedule.amount_per_run_sol} SOL</td>
                <td>{schedule.interval_secs}s</td>
                <td>{schedule.spent_sol} / {schedule.budget_sol} SOL</td>
                <td>
                  <span className="tag">{schedule.status}</span>
                  {schedule.last_error && <span title={schedule.last_error}> ⚠</span>}
                </td>
                <td>{new Date(schedule.next_run_at).toLocaleString()}</td>
                <td>
                  <div className="row">
                    <button type="button" className="link" onClick={() => onShowRuns(schedule)}>
                      Runs
                    </button>
                    {schedule.status === "active" && (
                      <button
                        type="button"
                        className="link"
                        disabled={!canManage || busy}
                        onClick={() => onPause(schedule)}
                      >
                        Pause
                      </button>
                    )}
                    {schedule.status === "paused" && (
                      <button
                        type="button"
                        className="link"
                        disabled={!canManage || busy}
                        onClick={() => onResume(schedule)}
                      >
                        Resume
                      </button>
                    )}
                    {(schedule.status === "active" || schedule.status === "paused") && (
                      <button
                        type="button"
                        className="link"
                        disabled={!canManage || busy}
                        onClick={() => onRequestCancel(schedule)}
                      >
                        Cancel
                      </button>
                    )}
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default ScheduleTable;
