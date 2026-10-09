import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Risk Disclosure",
};

/**
 * Risk Disclosure (GAP MAP v2, Part 5).
 *
 * The page a trading product cannot skip. Counsel-review placeholder, but the
 * risk statements themselves are final in intent: automated trading can lose
 * money fast, results shown anywhere are hypothetical, and past or simulated
 * performance never promises future results.
 */
export default function RiskDisclosurePage() {
  return (
    <article>
      <h1 style={{ marginTop: 0 }}>Risk Disclosure</h1>
      <p className="muted" style={{ fontSize: "0.85rem" }}>
        Draft for counsel review · version 2026-10-07.1 · read before enabling
        any live trading
      </p>

      <div
        className="card"
        style={{ borderColor: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}
      >
        <strong>Trading digital assets involves substantial risk of loss and is
        not suitable for everyone.</strong> Automated strategies can and do
        lose money, sometimes the entire deployed balance, within minutes.
        Never trade funds you cannot afford to lose.
      </div>

      <h2>1. Hypothetical results only</h2>
      <p>
        Any performance figure you see in this product — backtests, dashboards,
        marketing pages — is hypothetical or simulated unless explicitly tied
        to verifiable on-chain or venue records for your own account.
        Hypothetical performance has inherent limitations: it is prepared with
        the benefit of hindsight, it may not reflect actual execution,
        slippage, fees, or liquidity, and it is no indication of future
        results. No representation is made that any account will or is likely
        to achieve profits or losses similar to those shown.
      </p>

      <h2>2. Market and venue risk</h2>
      <p>
        Prices on decentralized exchanges, bonding curves, and prediction
        markets can be extremely volatile and illiquid. Orders may fail,
        partially fill, or execute at materially worse prices than expected.
        Smart-contract venues can suffer exploits, upgrades, or outages beyond
        our control.
      </p>

      <h2>3. Technology risk</h2>
      <p>
        Software bugs, connectivity loss, RPC failures, chain congestion, and
        key-management incidents can cause missed trades, unintended trades,
        or loss of funds. Kill-switch and risk-gate features reduce — they do
        not eliminate — these risks.
      </p>

      <h2>4. Regulatory risk</h2>
      <p>
        The legality of automated trading and of specific venues varies by
        jurisdiction and changes over time. You are responsible for
        determining that your use of the service complies with the laws that
        apply to you. [Counsel: add region-specific restrictions.]
      </p>

      <h2>5. Your acknowledgment</h2>
      <p>
        By enabling live trading you confirm that you have read this
        disclosure, you understand that losses are possible and can be total,
        and you accept responsibility for your configuration and funds.
        [Counsel: finalize acknowledgment mechanics and record-keeping.]
      </p>
    </article>
  );
}
