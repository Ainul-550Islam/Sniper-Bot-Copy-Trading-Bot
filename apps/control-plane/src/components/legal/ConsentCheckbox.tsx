"use client";

/**
 * Sign-up consent checkbox (GAP MAP v2, Part 5).
 *
 * Gate for the "Create account" flow: registration cannot be submitted until
 * the user ticks this box. The tick captures WHICH legal-bundle version was
 * accepted and WHEN (ISO timestamp); the parent form sends that record with
 * the registration request and the server stores it in the durable audit
 * trail (`saas.user.consent_recorded`).
 *
 * Unchecking withdraws the record (the parent receives `null` again) — we
 * never remember consent that was taken back before submission.
 */
import { captureConsent, CONSENT_VERSION, type ConsentRecord } from "@/lib/consent";

interface ConsentCheckboxProps {
  /** The current consent state, owned by the parent form. */
  consent: ConsentRecord | null;
  /** Called with a fresh record when ticked, `null` when unticked. */
  onChange: (consent: ConsentRecord | null) => void;
  disabled?: boolean;
}

const LINKS: Array<{ href: string; label: string }> = [
  { href: "/legal/terms", label: "Terms of Service" },
  { href: "/legal/privacy", label: "Privacy Policy" },
  { href: "/legal/risk-disclosure", label: "Risk Disclosure" },
];

export default function ConsentCheckbox({ consent, onChange, disabled }: ConsentCheckboxProps) {
  const checked = consent !== null;
  return (
    <div className="card" style={{ padding: "0.9rem 1rem", background: "var(--panel-2)" }}>
      <label style={{ display: "flex", gap: "0.6rem", alignItems: "flex-start", cursor: disabled ? "default" : "pointer" }}>
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked ? captureConsent() : null)}
          style={{ marginTop: "0.2rem" }}
          aria-describedby="consent-docs"
        />
        <span style={{ fontSize: "0.82rem", lineHeight: 1.5 }}>
          I have read and accept the{" "}
          {LINKS.map((link, index) => (
            <span key={link.href}>
              {index > 0 && (index < LINKS.length - 1 ? ", " : " and ")}
              <a href={link.href} target="_blank" rel="noopener noreferrer">
                {link.label}
              </a>
            </span>
          ))}
          . I understand that trading results shown anywhere in this product
          are hypothetical and that trading can result in total loss.
        </span>
      </label>
      <div id="consent-docs" className="muted" style={{ fontSize: "0.72rem", marginTop: "0.5rem" }}>
        Acceptance is recorded with document version{" "}
        <code>{consent ? consent.version : CONSENT_VERSION}</code>
        {consent ? <> at <code>{consent.accepted_at}</code></> : null}.
      </div>
    </div>
  );
}
