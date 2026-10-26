/**
 * Export download client (Part 04, L5).
 *
 * Performs an authenticated fetch through the shared `request` client
 * (session token and tenant id travel in headers — never in a URL or in
 * browser storage), turns the returned envelope into a `Blob`, and triggers
 * a same-origin download. No download URL ever carries credentials.
 */

import { request, type ExportEnvelope } from "../api";

/** Download a deterministic export envelope as a JSON blob file. */
export async function downloadExport(kind: string): Promise<void> {
  const envelope = await request<ExportEnvelope>(
    `/api/saas/exports?kind=${encodeURIComponent(kind)}`,
  );
  downloadJson(envelope, `sniper-export-${kind}.json`);
}

/** Download a generated report (or any JSON response) as a blob file. */
export async function downloadReport(reportId: string): Promise<void> {
  const report = await request<Record<string, unknown>>(
    `/api/saas/reports/${encodeURIComponent(reportId)}/download`,
  );
  downloadJson(report, `sniper-report-${reportId}.json`);
}

/** Shared blob download: no credentials touch the download URL. */
export async function downloadJson(payload: unknown, fileName: string): Promise<void> {
  const blob = new Blob([JSON.stringify(payload, null, 2)], {
    type: "application/json; charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.style.display = "none";
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  URL.revokeObjectURL(url);
}

export const exportsApi = {
  download: downloadExport,
  downloadReport,
  downloadJson,
};
