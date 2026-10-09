/** Framework loading state. It contains no business values or assumed tenant data. */
export default function LoadingPage() {
  return (
    <main
      aria-busy="true"
      style={{
        minHeight: "100vh",
        display: "grid",
        placeItems: "center",
        padding: "2rem",
        background: "var(--bg, #07111f)",
        color: "var(--muted, #9fb0c5)",
      }}
    >
      Loading control plane
    </main>
  );
}
