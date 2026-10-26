"use client";

import type { DcaSchedule } from "@/lib/api/dca-api";

interface CancelConfirmProps {
  schedule: DcaSchedule;
  busy: boolean;
  onConfirm: () => void;
  onDismiss: () => void;
}

/**
 * Kill-switch-style confirmation for DCA cancellation. Rendering this
 * component performs no request; only the explicit confirm button calls the
 * parent, which owns the API call and the subsequent refetch.
 */
export function CancelConfirm({ schedule, busy, onConfirm, onDismiss }: CancelConfirmProps) {
  return (
    <div className="card" role="alertdialog" aria-label="Confirm DCA cancellation" style={{ borderLeft: "4px solid var(--bad)" }}>
      <h3>Cancel this DCA schedule?</h3>
      <p className="muted">
        Schedule <code>{schedule.id}</code> for <code>{schedule.mint}</code> will stop recording
        future runs. This does not alter runs already recorded by the server.
      </p>
      <div className="row">
        <button type="button" onClick={onDismiss} disabled={busy}>
          Keep schedule
        </button>
        <button type="button" className="danger" onClick={onConfirm} disabled={busy}>
          {busy ? "Cancelling…" : "Confirm cancellation"}
        </button>
      </div>
    </div>
  );
}

export default CancelConfirm;
