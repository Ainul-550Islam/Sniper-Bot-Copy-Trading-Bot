"use client";

/**
 * Route-level failure boundary for the customer control plane.
 *
 * The boundary does not fabricate a successful response or silently retry a
 * mutating request. It shows the failure and lets the user request a fresh
 * render through Next's boundary reset.
 */

import { useEffect } from "react";

export default function ErrorPage({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <main
      role="alert"
      style={{
        minHeight: "100vh",
        display: "grid",
        placeItems: "center",
        padding: "2rem",
        background: "var(--bg, #07111f)",
        color: "var(--txt, #f4f7fb)",
      }}
    >
      <section style={{ width: "min(100%, 36rem)", textAlign: "center" }}>
        <p style={{ color: "#ffaaa0", fontWeight: 700 }}>Control-plane error</p>
        <h1>Something went wrong while rendering this page.</h1>
        <p style={{ color: "var(--muted, #9fb0c5)", lineHeight: 1.6 }}>
          No transaction or account state was assumed. Retry the page, and if the problem continues,
          provide the support team with the correlation details from the server response.
        </p>
        {error.digest ? (
          <p style={{ color: "var(--muted, #9fb0c5)", fontFamily: "monospace" }}>
            Reference: {error.digest}
          </p>
        ) : null}
        <button
          type="button"
          onClick={() => reset()}
          style={{
            marginTop: "1rem",
            padding: "0.75rem 1rem",
            border: 0,
            borderRadius: "8px",
            background: "#4f8cff",
            color: "white",
            fontWeight: 700,
            cursor: "pointer",
          }}
        >
          Try again
        </button>
      </section>
    </main>
  );
}
