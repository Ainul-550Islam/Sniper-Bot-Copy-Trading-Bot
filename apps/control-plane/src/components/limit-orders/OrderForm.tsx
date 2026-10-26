"use client";

import { useState } from "react";
import type {
  CreateLimitOrderInput,
  LimitOrderSide,
  LimitOrderTrigger,
} from "@/lib/api/limit-orders-api";

interface OrderFormProps {
  canManage: boolean;
  busy: boolean;
  onCreate: (input: CreateLimitOrderInput) => Promise<void>;
}

export function OrderForm({ canManage, busy, onCreate }: OrderFormProps) {
  const [mint, setMint] = useState("");
  const [side, setSide] = useState<LimitOrderSide>("buy");
  const [trigger, setTrigger] = useState<LimitOrderTrigger>("at_or_below");
  const [price, setPrice] = useState("");
  const [amount, setAmount] = useState("");
  const [expiresInSecs, setExpiresInSecs] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const priceSol = Number(price);
    const amountValue = Number(amount);
    const expiry = expiresInSecs.trim() === "" ? undefined : Number(expiresInSecs);

    if (!mint.trim()) {
      setError("Mint is required.");
      return;
    }
    if (!Number.isFinite(priceSol) || priceSol <= 0) {
      setError("Trigger price must be a positive number of SOL.");
      return;
    }
    if (!Number.isFinite(amountValue) || amountValue <= 0) {
      setError("Amount must be a positive number.");
      return;
    }
    if (side === "sell" && amountValue > 1) {
      setError("Sell amount is a fraction of the position and cannot exceed 1.");
      return;
    }
    if (expiry !== undefined && (!Number.isInteger(expiry) || expiry <= 0)) {
      setError("Expiry must be a positive whole number of seconds.");
      return;
    }

    setError(null);
    await onCreate({
      mint: mint.trim(),
      side,
      trigger,
      price_sol: priceSol,
      amount: amountValue,
      ...(expiry === undefined ? {} : { expires_in_secs: expiry }),
    });
  }

  return (
    <section className="card" aria-label="Create limit order">
      <h2>Create limit order</h2>
      {!canManage && (
        <p className="muted">Your current role can view limit orders but cannot create or cancel them.</p>
      )}
      <form onSubmit={(event) => void submit(event)} className="form">
        <div className="form-group">
          <label htmlFor="limit-mint">Token mint</label>
          <input id="limit-mint" value={mint} onChange={(event) => setMint(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <div className="form-group">
          <label htmlFor="limit-side">Side</label>
          <select id="limit-side" value={side} onChange={(event) => setSide(event.target.value as LimitOrderSide)} disabled={!canManage || busy}>
            <option value="buy">Buy</option>
            <option value="sell">Sell</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="limit-trigger">Trigger</label>
          <select id="limit-trigger" value={trigger} onChange={(event) => setTrigger(event.target.value as LimitOrderTrigger)} disabled={!canManage || busy}>
            <option value="at_or_below">At or below</option>
            <option value="at_or_above">At or above</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="limit-price">Trigger price (SOL)</label>
          <input id="limit-price" type="number" min="0" step="any" value={price} onChange={(event) => setPrice(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <div className="form-group">
          <label htmlFor="limit-amount">Amount</label>
          <input id="limit-amount" type="number" min="0" step="any" value={amount} onChange={(event) => setAmount(event.target.value)} disabled={!canManage || busy} required />
          {side === "sell" && (
            <p className="muted small">For a sell, amount is the fraction of the position to sell (up to 1).</p>
          )}
        </div>
        <div className="form-group">
          <label htmlFor="limit-expiry">Expires in (seconds, optional)</label>
          <input id="limit-expiry" type="number" min="1" step="1" value={expiresInSecs} onChange={(event) => setExpiresInSecs(event.target.value)} disabled={!canManage || busy} />
        </div>
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || busy}>
          {busy ? "Creating…" : "Create limit order"}
        </button>
      </form>
    </section>
  );
}

export default OrderForm;
