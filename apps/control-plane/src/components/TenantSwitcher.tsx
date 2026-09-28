"use client";

import { useMemo, useState } from "react";

import { ApiError } from "@/lib/api";

/**
 * The tenant switcher (TASK 7B file 09).
 *
 * The ONLY way to choose a tenant is to pick one of the organizations the
 * signed-in user actually belongs to (the server's own answer to
 * `GET /api/saas/users/me`). There is deliberately NO free-text
 * organization-id entry: a browser-supplied id is a hint at best, and the
 * backend re-authorizes every request regardless of what this widget sends.
 * The UI simply never offers anything beyond the user's own tenants.
 */

export interface TenantOption {
  organization_id: string;
  slug: string;
  name: string;
  status: string;
  role: string;
  membership_status: string;
}

export function TenantSwitcher({
  organizations,
  selected,
  onSelect,
}: {
  organizations: TenantOption[];
  selected: string | null;
  onSelect: (organizationId: string | null) => void;
}) {
  const usable = useMemo(
    () => organizations.filter((m) => m.membership_status === "active"),
    [organizations],
  );
  const current = organizations.find((m) => m.organization_id === selected) ?? null;
  const [open, setOpen] = useState(false);

  if (usable.length === 0) {
    return (
      <div className="tenant-switcher empty" role="note">
        No organization membership yet — create one under <em>Settings</em>.
      </div>
    );
  }

  return (
    <div className="tenant-switcher">
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="tenant-switcher__toggle"
      >
        <span className="tenant-switcher__label">
          {current ? (
            <>
              <strong>{current.name}</strong>{" "}
              <span className="muted">({current.role})</span>
            </>
          ) : (
            "Select organization…"
          )}
        </span>
        <span aria-hidden>▾</span>
      </button>

      {open && (
        <ul className="tenant-switcher__list" role="listbox" aria-label="Organizations">
          {current && (
            <li>
              <button
                type="button"
                role="option"
                aria-selected={current.organization_id === selected}
                onClick={() => {
                  onSelect(null);
                  setOpen(false);
                }}
              >
                — no tenant context —
              </button>
            </li>
          )}
          {usable.map((org) => (
            <li key={org.organization_id}>
              <button
                type="button"
                role="option"
                aria-selected={org.organization_id === selected}
                onClick={() => {
                  onSelect(org.organization_id);
                  setOpen(false);
                }}
              >
                <span>
                  <strong>{org.name}</strong>{" "}
                  <span className="muted small">/{org.slug}</span>
                </span>
                <span className={`tag tag--${org.status}`}>
                  {org.status} · {org.role}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Guard for sections that need a tenant context. */
export function TenantGate({
  selected,
  children,
}: {
  selected: string | null;
  children: React.ReactNode;
}) {
  if (!selected) {
    return (
      <p className="notice" role="note">
        Select an organization to view this section.
      </p>
    );
  }
  return <>{children}</>;
}

export { ApiError };
