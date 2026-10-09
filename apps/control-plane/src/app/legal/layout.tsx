/**
 * Layout for the public legal pages (GAP MAP v2, Part 5).
 *
 * These pages are unauthenticated: a prospective customer must be able to
 * read the terms BEFORE creating an account. They render as plain documents
 * with no AppShell (no tenant context, no session), and every page carries
 * the same mandatory disclaimer header so no page can be read as a
 * performance promise.
 */
import Link from "next/link";

export default function LegalLayout({ children }: { children: React.ReactNode }) {
  return (
    <main
      style={{
        maxWidth: "760px",
        margin: "0 auto",
        padding: "2.5rem 1.25rem 4rem",
        color: "var(--text)",
        lineHeight: 1.6,
      }}
    >
      <header style={{ marginBottom: "1.5rem" }}>
        <Link href="/" style={{ color: "var(--muted)", fontSize: "0.85rem", textDecoration: "none" }}>
          ← Back to Sniper Suite
        </Link>
      </header>
      {children}
      <footer style={{ marginTop: "3rem", paddingTop: "1rem", borderTop: "1px solid var(--panel-2)", color: "var(--muted)", fontSize: "0.8rem" }}>
        <p style={{ margin: 0 }}>
          These documents are working drafts pending review by qualified
          counsel and are provided for information only. Nothing on this site
          is financial, legal, or tax advice.
        </p>
      </footer>
    </main>
  );
}
