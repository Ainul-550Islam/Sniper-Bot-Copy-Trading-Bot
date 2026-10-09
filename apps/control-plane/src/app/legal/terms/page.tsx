import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Terms of Service",
};

/**
 * Terms of Service (GAP MAP v2, Part 5).
 *
 * Deliberately a counsel-review placeholder: it states the operative shape of
 * the agreement and the non-negotiable risk language, but every substantive
 * clause is flagged for legal review rather than asserting finalized terms.
 * It makes NO performance, uptime, or profit guarantee anywhere.
 */
export default function TermsPage() {
  return (
    <article>
      <h1 style={{ marginTop: 0 }}>Terms of Service</h1>
      <p className="muted" style={{ fontSize: "0.85rem" }}>
        Draft for counsel review · version 2026-10-07.1 · not legal advice
      </p>

      <div
        className="card"
        style={{ borderColor: "var(--warn)", background: "var(--warn-glow)", marginBottom: "1.5rem" }}
      >
        <strong>No guarantee of results.</strong> Sniper Suite is software that
        helps configure, run, and monitor automated trading strategies. Any
        results shown anywhere in our materials are hypothetical and
        illustrative only. We do not guarantee profits, and you can lose some
        or all of the funds you trade.
      </div>

      <h2>1. The service</h2>
      <p>
        Sniper Suite provides a multi-tenant control plane for operating
        automated trading strategies across supported venues. Access is
        licensed, not sold, on a subscription basis. [Counsel: insert precise
        scope-of-license, permitted use, and restrictions.]
      </p>

      <h2>2. No investment advice</h2>
      <p>
        Nothing in the service constitutes investment, legal, or tax advice.
        You are solely responsible for your trading decisions and for
        complying with the laws and venue rules that apply to you. [Counsel:
        add jurisdiction-specific disclaimers.]
      </p>

      <h2>3. Hypothetical performance</h2>
      <p>
        Backtests and simulated results are produced from historical or
        synthetic data, are clearly labeled as such, and carry substantial
        limitations — including survivorship and look-ahead bias. They are not
        indicative of future results. [Counsel: expand per CFTC/NFA-style
        hypothetical-performance language where applicable.]
      </p>

      <h2>4. Your responsibilities</h2>
      <p>
        You are responsible for keeping credentials confidential, for the
        accuracy of configuration you provide, and for monitoring live
        activity. [Counsel: define acceptable use, prohibited conduct, and
        suspension rights.]
      </p>

      <h2>5. Fees, billing, and termination</h2>
      <p>
        Fees are described at checkout and are billed in advance. [Counsel:
        insert refund policy, renewal terms, and termination/ suspension
        mechanics, including the kill-switch and suspension behavior.]
      </p>

      <h2>6. Disclaimers and limitation of liability</h2>
      <p>
        The service is provided “as is” and “as available.” To the maximum
        extent permitted by law, we disclaim all warranties and limit our
        liability. [Counsel: finalize enforceable disclaimer and
        limitation-of-liability language for target jurisdictions.]
      </p>

      <h2>7. Changes</h2>
      <p>
        We may update these terms; material changes take effect after notice.
        The version string above identifies the revision you accepted at
        sign-up. [Counsel: define notice method and continued-use consent.]
      </p>
    </article>
  );
}
