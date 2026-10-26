"use client";

/**
 * Customer-Facing Service Status Page (THIRD.md §112).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import ServiceStatusGrid from "@/components/status/ServiceStatusGrid";
import { ErrorState } from "@/components/common/ErrorState";
import { ServiceStatusReport, getServiceStatus } from "@/lib/api/status-api";
import { request, ApiError, toDisplayError } from "@/lib/api";

export default function StatusPage() {
  const [report, setReport] = useState<ServiceStatusReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [backup, setBackup] = useState<{
    backup_count: number;
    last_backup_at: string | null;
    last_verified_restore: string | null;
    encrypted: boolean | null;
    offsite: { state: string; last_sync_at: string | null } | null;
    as_of: string;
  } | null>(null);
  const [backupLoading, setBackupLoading] = useState(false);
  const [backupError, setBackupError] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getServiceStatus();
      setReport(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load service status");
    } finally {
      setLoading(false);
    }
  }, []);

  const loadBackup = useCallback(async () => {
    setBackupLoading(true);
    setBackupError(null);
    try {
      const data = await request<{
        backup_count: number;
        last_backup_at: string | null;
        last_verified_restore: string | null;
        encrypted: boolean | null;
        offsite: { state: string; last_sync_at: string | null } | null;
        as_of: string;
      }>("/api/saas/backup/status");
      setBackup(data);
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setBackup(null);
      } else {
        setBackupError(toDisplayError(err));
      }
    } finally {
      setBackupLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => {
      void loadStatus();
      void loadBackup();
    });
  }, [loadStatus, loadBackup]);

  return (
    <AppShell title="System Status">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>System Health &amp; Infrastructure Status</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Real-time telemetry on execution relayers, Geyser streams, KMS signers, and storage clusters.
          </p>
        </div>
        <button onClick={() => void loadStatus()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Telemetry
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadStatus()} />}

      {loading ? (
        <div className="card">Loading telemetry status...</div>
      ) : report ? (
        <div>
          <div
            className="card"
            style={{
              borderColor: report.overall_status === "operational" ? "var(--ok)" : "var(--warn)",
              background: report.overall_status === "operational" ? "var(--ok-glow)" : "var(--warn-glow)",
              marginBottom: "1.5rem",
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
            }}
          >
            <div>
              <h3 style={{ margin: 0, color: report.overall_status === "operational" ? "var(--ok)" : "var(--warn)" }}>
                {report.overall_status === "operational" ? "All registered components operational" : `Health registry: ${report.overall_status}`}
              </h3>
              <p style={{ margin: "0.25rem 0 0", fontSize: "0.85rem" }}>
                {report.components.length === 0
                  ? "No health components are registered in this deployment."
                  : report.active_incidents_count === null
                    ? "Incident count unavailable."
                    : `${report.active_incidents_count} registered component incident${report.active_incidents_count === 1 ? "" : "s"}.`}
              </p>
            </div>
            <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>
              As of {new Date(report.as_of).toLocaleTimeString()}
            </div>
          </div>

          {/* Backup status: what the append-only backup ledger can prove */}
          <section className="card" style={{ marginTop: "1.5rem" }} aria-label="Backup status">
            <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
              <h3 style={{ margin: 0 }}>Backup Status</h3>
              <p style={{ margin: "0.25rem 0 0", fontSize: "0.85rem", color: "var(--muted)" }}>
                Reports only what the append-only backup ledger proves.
              </p>
            </div>
            {backupLoading ? (
              <p style={{ padding: "1rem", color: "var(--muted)" }}>Loading backup status…</p>
            ) : backupError ? (
              <div style={{ padding: "1rem", color: "var(--bad)" }}>{backupError}</div>
            ) : backup === null ? (
              <p style={{ padding: "1rem", color: "var(--muted)" }}>
                No backup status was returned for this deployment.
              </p>
            ) : (
              <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "0.5rem" }}>
                <tbody>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Backups</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.backup_count}</td>
                  </tr>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Last Backup</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.last_backup_at ?? "—"}</td>
                  </tr>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Last Verified Restore</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.last_verified_restore ?? "—"}</td>
                  </tr>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Encrypted at Rest</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.encrypted != null ? String(backup.encrypted) : "—"}</td>
                  </tr>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Offsite State</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.offsite ? backup.offsite.state : "—"}</td>
                  </tr>
                  <tr style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><strong>Offsite Last Sync</strong></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{backup.offsite ? backup.offsite.last_sync_at ?? "—" : "—"}</td>
                  </tr>
                </tbody>
              </table>
            )}
            <p style={{ marginTop: "0.75rem", fontSize: "0.8rem", color: "var(--muted)" }}>
              As of {backup?.as_of ?? "—"}.
            </p>
          </section>

          <ServiceStatusGrid components={report.components} />
        </div>
      ) : null}
    </AppShell>
  );
}
