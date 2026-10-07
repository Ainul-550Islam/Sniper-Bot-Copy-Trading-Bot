"use client";

/**
 * Compliance, Accounting & Execution Reports Console (SECOND.md §65).
 *
 * Generate tenant-scoped audit trails, accounting ledgers, and execution reports
 * in the formats supported by the durable report service.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import ReportTable, { ReportItem } from "@/components/reports/report-table";
import { request } from "@/lib/api";

export default function ReportsPage() {
  const [reports, setReports] = useState<ReportItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadReports = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await request<{ items: ReportItem[] }>("/api/saas/reports");
      setReports(res.items);
    } catch (err: unknown) {
      setReports([]);
      setError(err instanceof Error ? err.message : "Failed to load reports");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadReports());
  }, [loadReports]);

  return (
    <AppShell title="Reports & Exports">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Compliance, Tax &amp; Audit Reports</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Export deterministic transaction ledgers, realized gains, and member authorization trails.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      {loading ? (
        <div className="card">Loading compliance exports...</div>
      ) : (
        <ReportTable reports={reports} onRefresh={() => void loadReports()} />
      )}
    </AppShell>
  );
}
