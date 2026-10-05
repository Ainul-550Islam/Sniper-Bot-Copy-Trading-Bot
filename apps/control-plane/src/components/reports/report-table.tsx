"use client";

import { useState } from "react";
import { request } from "@/lib/api";

export interface ReportItem {
  id: string;
  name: string;
  type: string;
  format: string;
  size_bytes: number;
  record_count: number;
  created_at: string;
  download_url: string;
}

interface ReportTableProps {
  reports: ReportItem[];
  onRefresh: () => void;
}

export default function ReportTable({ reports, onRefresh }: ReportTableProps) {
  const [reportType, setReportType] = useState("accounting_ledger");
  const [format, setFormat] = useState("csv");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleGenerate = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);

    const now = new Date();
    const from = new Date(now.getTime() - 90 * 24 * 60 * 60 * 1000).toISOString();
    const to = now.toISOString();

    try {
      await request("/api/saas/reports/export", {
        method: "POST",
        body: {
          report_type: reportType,
          date_from: from,
          date_to: to,
          format,
        },
      });
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to generate report export");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div>
      <div className="card" style={{ marginBottom: "1.5rem" }}>
        <h3 style={{ marginTop: 0 }}>Generate Compliance &amp; Accounting Export</h3>
        {error && (
          <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
            {error}
          </div>
        )}

        <form onSubmit={handleGenerate} style={{ display: "flex", gap: "1rem", alignItems: "flex-end", flexWrap: "wrap" }}>
          <div style={{ flex: 2, minWidth: "220px" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Report Type
            </label>
            <select
              value={reportType}
              onChange={(e) => setReportType(e.target.value)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="accounting_ledger">Capital Gains &amp; Execution Ledger</option>
              <option value="security_audit">SOC2 Audit &amp; Member Access Trail</option>
              <option value="risk_events">Risk Limit Breaches &amp; Circuit Breakers</option>
              <option value="custody_rotation">Custody Key Lifecycle &amp; Signer Log</option>
            </select>
          </div>

          <div style={{ flex: 1, minWidth: "120px" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Format
            </label>
            <select
              value={format}
              onChange={(e) => setFormat(e.target.value)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="csv">CSV Spreadsheet</option>
              <option value="json">JSON Ledger</option>
              <option value="pdf">PDF Statement</option>
            </select>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary" style={{ height: "38px" }}>
            {loading ? "Exporting..." : "Generate Export"}
          </button>
        </form>
      </div>

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <h3 style={{ margin: 0 }}>Available Generated Reports ({reports.length})</h3>
        </div>

        {reports.length === 0 ? (
          <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
            No compliance exports requested yet.
          </div>
        ) : (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Report Name</th>
                <th style={{ padding: "0.75rem 1rem" }}>Type</th>
                <th style={{ padding: "0.75rem 1rem" }}>Format</th>
                <th style={{ padding: "0.75rem 1rem" }}>Records / Size</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created Date</th>
                <th style={{ padding: "0.75rem 1rem" }}>Download</th>
              </tr>
            </thead>
            <tbody>
              {reports.map((r) => (
                <tr key={r.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <strong>{r.name}</strong>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {r.type}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className="badge" style={{ textTransform: "uppercase" }}>{r.format}</span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                    {r.record_count.toLocaleString()} rows ({(r.size_bytes / 1024).toFixed(1)} KB)
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {new Date(r.created_at).toLocaleDateString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <a
                      href={r.download_url}
                      className="btn btn-secondary"
                      style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem" }}
                      download
                    >
                      Download
                    </a>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
