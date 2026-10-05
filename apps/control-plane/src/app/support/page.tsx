"use client";

/**
 * Tenant Support Center & Incident Filing Page (THIRD.md §109).
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import SupportTicketTable from "@/components/support/SupportTicketTable";
import { ErrorState } from "@/components/common/ErrorState";
import { SupportTicket, TicketPriority, createSupportTicket, listSupportTickets } from "@/lib/api/support-api";

export default function SupportPage() {
  const [tickets, setTickets] = useState<SupportTicket[]>([]);
  const [subject, setSubject] = useState("");
  const [priority, setPriority] = useState<TicketPriority>("medium");
  const [description, setDescription] = useState("");
  const [showForm, setShowForm] = useState(false);
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await listSupportTickets();
      setTickets(res.items || []);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load support tickets");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await createSupportTicket({ subject, priority, description });
      setSubject("");
      setDescription("");
      setShowForm(false);
      void loadData();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to create ticket");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AppShell title="Support & Incident SLA">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Enterprise Support &amp; Incident Center</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            File priority incident reports and track institutional SLA resolution workflows.
          </p>
        </div>
        <button onClick={() => setShowForm(!showForm)} className="btn btn-primary">
          {showForm ? "Close Form" : "+ File Incident Ticket"}
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadData()} />}

      {showForm && (
        <div className="card" style={{ marginBottom: "1.5rem" }}>
          <h3 style={{ marginTop: 0 }}>File Incident / Support Ticket</h3>
          <form onSubmit={handleSubmit}>
            <div style={{ display: "grid", gridTemplateColumns: "3fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
              <div>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Subject
                </label>
                <input
                  type="text"
                  required
                  value={subject}
                  onChange={(e) => setSubject(e.target.value)}
                  placeholder="e.g. Jito RPC bundle latency degradation on Frankfurt relay"
                  className="input"
                  style={{ width: "100%" }}
                />
              </div>

              <div>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Severity / SLA
                </label>
                <select
                  value={priority}
                  onChange={(e) => setPriority(e.target.value as TicketPriority)}
                  className="select"
                  style={{ width: "100%" }}
                >
                  <option value="low">Low (Standard)</option>
                  <option value="medium">Medium (4h SLA)</option>
                  <option value="high">High (1h SLA)</option>
                  <option value="urgent">Urgent / Critical (15m SLA)</option>
                </select>
              </div>
            </div>

            <div style={{ marginBottom: "1rem" }}>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Incident Details &amp; Correlation Logs
              </label>
              <textarea
                rows={4}
                required
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Include order IDs, bot runtime generations, or bundle transaction signatures..."
                className="input"
                style={{ width: "100%" }}
              />
            </div>

            <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem" }}>
              <button type="button" onClick={() => setShowForm(false)} className="btn btn-secondary">
                Cancel
              </button>
              <button type="submit" disabled={submitting} className="btn btn-primary">
                {submitting ? "Submitting..." : "Submit Incident Ticket"}
              </button>
            </div>
          </form>
        </div>
      )}

      {loading ? (
        <div className="card">Loading support tickets...</div>
      ) : (
        <SupportTicketTable tickets={tickets} />
      )}
    </AppShell>
  );
}
