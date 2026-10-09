"use client";

/**
 * Guided Strategy Creation Flow (THIRD.md §104).
 */

import { useRouter } from "next/navigation";
import AppShell from "@/components/AppShell";
import StrategyForm from "@/components/strategy/strategy-form";

export default function NewStrategyPage() {
  const router = useRouter();

  return (
    <AppShell title="Create Strategy">
      <div style={{ maxWidth: "800px", margin: "0 auto" }}>
        <div style={{ marginBottom: "1.5rem" }}>
          <h1 style={{ margin: 0 }}>Create New Trading Strategy</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Configure autonomous execution rules with typed parameter validation and anti-rug safeguards.
          </p>
        </div>

        <StrategyForm
          onSuccess={() => router.push("/strategies")}
          onCancel={() => router.push("/strategies")}
        />
      </div>
    </AppShell>
  );
}
