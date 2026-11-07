"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";

import {
  clearSession,
  establishSession,
  refresh,
  selectOrganization,
  sessionStore,
} from "@/lib/auth";
import { toDisplayError } from "@/lib/api";
import {
  completeSsoLogin,
  PENDING_SSO_STATE_KEY,
} from "@/lib/api/sso-api";

export default function SsoCallbackPage() {
  const router = useRouter();
  const [message, setMessage] = useState("Completing your sign-in…");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    async function finishLogin() {
      const callbackUrl = new URL(window.location.href);
      const returnedState = callbackUrl.searchParams.get("state");
      const code = callbackUrl.searchParams.get("code");
      const providerError = callbackUrl.searchParams.get("error");
      const expectedState = window.sessionStorage.getItem(PENDING_SSO_STATE_KEY);

      // Remove authorization material from the address bar/history immediately.
      window.history.replaceState({}, document.title, "/sso/callback");
      window.sessionStorage.removeItem(PENDING_SSO_STATE_KEY);

      if (providerError) {
        throw new Error("The identity provider did not complete sign-in.");
      }
      if (!returnedState || !expectedState || returnedState !== expectedState) {
        throw new Error("The SSO sign-in state was missing or did not match. Please try again.");
      }
      if (!code) {
        throw new Error("The identity provider returned no authorization code.");
      }

      let sessionEstablished = false;
      try {
        const response = await completeSsoLogin(returnedState, code);
        if (!active) return;
        establishSession(response);
        sessionEstablished = true;

        if (response.mfa_enrollment_required) {
          router.replace("/mfa-enrollment");
          return;
        }

        if (!(await refresh())) {
          throw new Error("Your SSO session could not be verified. Please try again.");
        }
        if (!active) return;

        const loginOrganization = response.session.organization_id ?? response.organization_id ?? null;
        const organizations = sessionStore.getSnapshot().organizations;
        if (loginOrganization && organizations.some(
          (membership) => membership.organization_id === loginOrganization,
        )) {
          selectOrganization(loginOrganization);
        } else if (organizations.length === 1) {
          selectOrganization(organizations[0]!.organization_id);
        }
        router.replace("/");
      } catch (cause: unknown) {
        if (sessionEstablished) clearSession("expired");
        throw cause;
      }
    }

    void Promise.resolve().then(() => {
      if (active) return finishLogin();
    }).catch((cause: unknown) => {
      if (!active) return;
      setError(toDisplayError(cause));
      setMessage("Sign-in could not be completed.");
    });

    return () => {
      active = false;
    };
  }, [router]);

  return (
    <main className="landing">
      <section className="card" aria-live="polite">
        <h1>Single sign-on</h1>
        <p>{error ?? message}</p>
        {error ? (
          <button className="primary" type="button" onClick={() => router.replace("/")}>
            Return to sign-in
          </button>
        ) : null}
      </section>
    </main>
  );
}
