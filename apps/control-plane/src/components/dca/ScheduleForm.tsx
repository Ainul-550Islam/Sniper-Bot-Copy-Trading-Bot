"use client";

import { useState } from "react";
import type { CreateDcaScheduleInput } from "@/lib/api/dca-api";

interface ScheduleFormProps {
  canManage: boolean;
  busy: boolean;
  onCreate: (input: CreateDcaScheduleInput) => Promise<void>;
}

export function ScheduleForm({ canManage, busy, onCreate }: ScheduleFormProps) {
  const [mint, setMint] = useState("");
  const [amountPerRun, setAmountPerRun] = useState("");
  const [intervalSecs, setIntervalSecs] = useState("");
  const [budget, setBudget] = useState("");
  const [startInSecs, setStartInSecs] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const amount = Number(amountPerRun);
    const interval = Number(intervalSecs);
    const budgetSol = Number(budget);
    const startIn = startInSecs.trim() === "" ? undefined : Number(startInSecs);

    if (!mint.trim()) {
      setError("Mint is required.");
      return;
    }
    if (!Number.isFinite(amount) || amount <= 0) {
      setError("Amount per run must be a positive number.");
      return;
    }
    if (!Number.isInteger(interval) || interval <= 0) {
      setError("Interval must be a positive whole number of seconds.");
      return;
    }
    if (!Number.isFinite(budgetSol) || budgetSol < amount) {
      setError("Budget must cover at least one run.");
      return;
    }
    if (startIn !== undefined && (!Number.isInteger(startIn) || startIn < 0)) {
      setError("Start delay must be a non-negative whole number of seconds.");
      return;
    }

    setError(null);
    await onCreate({
      mint: mint.trim(),
      amount_per_run_sol: amount,
      interval_secs: interval,
      budget_sol: budgetSol,
      ...(startIn === undefined ? {} : { start_in_secs: startIn }),
    });
  }

  return (
    <section className="card" aria-label="Create DCA schedule">
      <h2>Create schedule</h2>
      {!canManage && (
        <p className="muted">Your current role can view schedules but cannot create or change them.</p>
      )}
      <form onSubmit={(event) => void submit(event)} className="form">
        <div className="form-group">
          <label htmlFor="dca-mint">Token mint</label>
          <input
            id="dca-mint"
            value={mint}
            onChange={(event) => setMint(event.target.value)}
            disabled={!canManage || busy}
            required
          />
        </div>
        <div className="form-group">
          <label htmlFor="dca-amount">Amount per run (SOL)</label>
          <input
            id="dca-amount"
            type="number"
            min="0"
            step="any"
            value={amountPerRun}
            onChange={(event) => setAmountPerRun(event.target.value)}
            disabled={!canManage || busy}
            required
          />
        </div>
        <div className="form-group">
          <label htmlFor="dca-interval">Interval (seconds)</label>
          <input
            id="dca-interval"
            type="number"
            min="1"
            step="1"
            value={intervalSecs}
            onChange={(event) => setIntervalSecs(event.target.value)}
            disabled={!canManage || busy}
            required
          />
        </div>
        <div className="form-group">
          <label htmlFor="dca-budget">Total budget (SOL)</label>
          <input
            id="dca-budget"
            type="number"
            min="0"
            step="any"
            value={budget}
            onChange={(event) => setBudget(event.target.value)}
            disabled={!canManage || busy}
            required
          />
        </div>
        <div className="form-group">
          <label htmlFor="dca-start-in">Start delay (seconds, optional)</label>
          <input
            id="dca-start-in"
            type="number"
            min="0"
            step="1"
            value={startInSecs}
            onChange={(event) => setStartInSecs(event.target.value)}
            disabled={!canManage || busy}
          />
        </div>
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || busy}>
          {busy ? "Creating…" : "Create DCA schedule"}
        </button>
      </form>
    </section>
  );
}

export default ScheduleForm;
