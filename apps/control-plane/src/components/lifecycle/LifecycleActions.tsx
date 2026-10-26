"use client";

import { TypedConfirm } from "./TypedConfirm";

export type LifecycleAction =
  | "disconnect"
  | "revoke-credentials"
  | "revoke-sessions"
  | "schedule-retention";

interface LifecycleActionsProps {
  organizationId: string;
  canManage: boolean;
  busyAction: LifecycleAction | null;
  onAction: (action: LifecycleAction) => void;
}

const ACTIONS: Array<{
  action: LifecycleAction;
  title: string;
  description: string;
  expected: string;
  confirmLabel: string;
}> = [
  {
    action: "disconnect",
    title: "Disconnect live sockets",
    description:
      "Disconnects this organization's live websocket sessions. The lifecycle status is refetched from the server after the action.",
    expected: "DISCONNECT",
    confirmLabel: "Disconnect sockets",
  },
  {
    action: "revoke-credentials",
    title: "Revoke custody credentials",
    description:
      "Marks this organization's custody credentials revoked in the lifecycle record. This does not display or transmit any credential material.",
    expected: "REVOKE",
    confirmLabel: "Revoke credentials",
  },
  {
    action: "revoke-sessions",
    title: "Revoke sessions",
    description:
      "Invalidates this organization's sessions according to the server lifecycle policy. You may need to sign in again.",
    expected: "SESSIONS",
    confirmLabel: "Revoke sessions",
  },
  {
    action: "schedule-retention",
    title: "Schedule retention",
    description:
      "Schedules the server-defined retention deadline. Repeating the action is handled idempotently by the server; the displayed deadline comes from a status refetch.",
    expected: "RETENTION",
    confirmLabel: "Schedule retention",
  },
];

export function LifecycleActions({
  organizationId,
  canManage,
  busyAction,
  onAction,
}: LifecycleActionsProps) {
  return (
    <section className="card" aria-label="Data lifecycle actions">
      <h2>Lifecycle actions</h2>
      <p className="muted small">
        Actions apply to organization <code>{organizationId}</code>. Each action requires a
        separate typed confirmation, and the status above is refreshed from the server after
        the selected POST completes.
      </p>
      {!canManage && (
        <p className="muted">Your current role can view lifecycle status but cannot run these actions.</p>
      )}
      <div className="stack" style={{ marginTop: "1rem" }}>
        {ACTIONS.map((item) => (
          <TypedConfirm
            key={item.action}
            title={item.title}
            description={item.description}
            expected={item.expected}
            confirmLabel={item.confirmLabel}
            busy={busyAction === item.action}
            disabled={!canManage || busyAction !== null}
            onConfirm={() => onAction(item.action)}
          />
        ))}
      </div>
    </section>
  );
}

export default LifecycleActions;
