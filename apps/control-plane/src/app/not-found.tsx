import Link from "next/link";

/** Honest framework-level 404 page; it does not redirect to a guessed tenant route. */
export default function NotFoundPage() {
  return (
    <main
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
        <p style={{ color: "var(--muted, #9fb0c5)", fontFamily: "monospace" }}>404</p>
        <h1>Page not found</h1>
        <p style={{ color: "var(--muted, #9fb0c5)", lineHeight: 1.6 }}>
          The requested control-plane route does not exist or is not published by this deployment.
        </p>
        <Link href="/" style={{ color: "#8db6ff", fontWeight: 700 }}>
          Return to the control plane
        </Link>
      </section>
    </main>
  );
}
