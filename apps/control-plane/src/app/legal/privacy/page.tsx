import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Privacy Policy",
};

/**
 * Privacy Policy (GAP MAP v2, Part 5).
 *
 * Counsel-review placeholder. It documents the data flows the system actually
 * performs (so the draft is grounded in reality, not invented) and flags each
 * legal section for completion. It must not overstate protections that have
 * not been implemented or certified.
 */
export default function PrivacyPage() {
  return (
    <article>
      <h1 style={{ marginTop: 0 }}>Privacy Policy</h1>
      <p className="muted" style={{ fontSize: "0.85rem" }}>
        Draft for counsel review · version 2026-10-07.1 · not legal advice
      </p>

      <h2>1. What we collect</h2>
      <p>
        Account data you provide (email, display name, organization name),
        configuration you enter (strategy settings, integrations you connect),
        and operational logs needed to run and secure the service. [Counsel:
        enumerate categories and legal bases per applicable regime.]
      </p>

      <h2>2. What we do not do</h2>
      <p>
        Session tokens are held in browser memory only and are not persisted to
        browser storage by the control plane. We do not sell personal data.
        [Counsel: verify each negative claim against the implementation before
        publication and add analytics/cookie disclosures if added later.]
      </p>

      <h2>3. Secrets and credentials</h2>
      <p>
        Trading credentials and signing keys you connect are stored encrypted
        and are used only to act on your instructions. [Counsel: describe the
        custody model, encryption-at-rest, and any third-party KMS/Vault
        processors accurately — do not claim certifications we do not hold.]
      </p>

      <h2>4. Sharing and processors</h2>
      <p>
        Data is shared only with processors required to provide the service
        (hosting, billing, email delivery) under appropriate agreements.
        [Counsel: list subprocessors and add cross-border transfer language.]
      </p>

      <h2>5. Retention and your rights</h2>
      <p>
        You can request access, correction, or deletion of your data. Data-lifecycle
        controls in the console let you disconnect integrations and schedule
        retention. [Counsel: finalize rights, retention periods, and contact
        details, including a data-protection contact.]
      </p>

      <h2>6. Security</h2>
      <p>
        We apply defense-in-depth measures appropriate to a trading system.
        [Counsel: describe measures truthfully; never reference security
        certifications, compliance attestations, or hardened key-management
        hardware that we have not actually obtained and can currently prove.]
      </p>

      <h2>7. Changes</h2>
      <p>
        We will give notice of material changes before they take effect.
        [Counsel: define the notice mechanism.]
      </p>
    </article>
  );
}
