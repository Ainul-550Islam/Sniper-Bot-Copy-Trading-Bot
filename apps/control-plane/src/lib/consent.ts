/**
 * Sign-up consent versioning (GAP MAP v2, Part 5).
 *
 * The control plane asks every new account to accept the legal bundle before
 * the "Create account" call is made. What the user accepted — WHICH version
 * of the documents and WHEN — travels with the registration request and is
 * recorded server-side in the durable audit trail
 * (`saas.user.consent_recorded`).
 *
 * Bump `CONSENT_VERSION` whenever the legal pages change materially; the
 * audit log keeps the version string forever, so historical signups stay
 * attributable to the exact wording they saw.
 */

/** Version of the legal bundle currently presented at sign-up. */
export const CONSENT_VERSION = "2026-10-07.1";

/** The consent record sent with `POST /api/saas/users`. */
export interface ConsentRecord {
  /** Legal bundle version accepted (see CONSENT_VERSION). */
  version: string;
  /** ISO-8601 timestamp of when the user ticked the box. */
  accepted_at: string;
}

/** Build the consent record at the moment the box is ticked. */
export function captureConsent(): ConsentRecord {
  return {
    version: CONSENT_VERSION,
    accepted_at: new Date().toISOString(),
  };
}
