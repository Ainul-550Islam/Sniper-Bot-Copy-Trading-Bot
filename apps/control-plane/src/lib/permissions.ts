/**
 * Shared Frontend Capability & Permission Display Model (THIRD.md §132).
 *
 * NOTE: This is a client-side display helper only. The backend SaaS
 * middleware remains the authoritative security and entitlement boundary.
 */

export type MembershipRole = "owner" | "admin" | "operator" | "analyst" | "viewer";

export interface TenantContext {
  role: MembershipRole;
  planCode: string;
  isSuspended: boolean;
  entitlementsActive: boolean;
}

export interface UserCapabilities {
  canManageTeam: boolean;
  canConfigureRisk: boolean;
  canExecuteEmergencyStop: boolean;
  canCreateStrategies: boolean;
  canDeployBots: boolean;
  canExportReports: boolean;
  canManageApiKeys: boolean;
  canManageBilling: boolean;
}

/**
 * Operator surface permissions.
 *
 * VISIBILITY ONLY — this is NOT the authorization boundary and it never grants
 * access to a single operator route.
 *
 * Operator authority on the backend is a platform operator key: `require_role`
 * authenticates the presented `x-api-key` against the operator key registry and
 * authorizes by the ROLE BUNDLED WITH THAT KEY (owner ⊃ operator ⊃ readonly).
 * A tenant membership role is not consulted anywhere on that path, so mapping
 * `owner`/`admin`/`operator` memberships onto the operator console here was a
 * lie in both directions: it promised tenant admins an operator console that
 * answers 401 on every call, and it hid the real requirement (a key).
 *
 * The rule is therefore: the Operator surface is shown to platform
 * administrators, and unlocking it requires an operator API key, which is
 * enforced by the backend on every request.
 */
export interface OperatorPermissions {
  /** Access to view operator console overview, metrics, logs, findings, and configuration. */
  read: boolean;
  /** Access to perform mutating operator actions (kill-switch, mode changes, module toggles, key management). */
  control: boolean;
}

export interface AppPermissions {
  operator: OperatorPermissions;
}

/**
 * Evaluates whether the operator surface is VISIBLE to this user.
 *
 * `role` is accepted for call-site compatibility and intentionally ignored: a
 * tenant membership role (owner, admin, operator, …) is a TENANT capability and
 * carries no operator authority. See the note on {@link OperatorPermissions}.
 *
 * `read` gates navigation and the guard; `control` gates the mutating controls.
 * Both require platform_admin, and both are re-proven by the backend on every
 * call from the operator key's own role.
 */
export function getOperatorPermissions(
  user?: { platform_admin?: boolean } | null,
  role?: string | null,
  isSuspended?: boolean,
): OperatorPermissions {
  void role; // A tenant membership role is NOT operator authority — see above.
  const isPlatformAdmin = Boolean(user?.platform_admin);
  const allowed = isPlatformAdmin && !isSuspended;

  return {
    read: allowed,
    control: allowed,
  };
}

/**
 * Resolves current permissions for the active session context.
 */
export function getPermissions(
  ctx?: Partial<TenantContext> | null,
  user?: { platform_admin?: boolean } | null,
): AppPermissions {
  return {
    operator: getOperatorPermissions(user, ctx?.role, ctx?.isSuspended),
  };
}

/** Evaluates user role and tenant context to compute UI button/tab visibility. */
export function getCapabilities(ctx?: Partial<TenantContext> | null): UserCapabilities {
  const role = ctx?.role ?? "viewer";
  const active = ctx?.entitlementsActive !== false && !ctx?.isSuspended;

  const isOwnerOrAdmin = role === "owner" || role === "admin";
  const isOperator = isOwnerOrAdmin || role === "operator";
  const isAnalyst = isOperator || role === "analyst";

  return {
    canManageTeam: isOwnerOrAdmin && active,
    canConfigureRisk: isOwnerOrAdmin && active,
    canExecuteEmergencyStop: isOperator && active,
    canCreateStrategies: isOperator && active,
    canDeployBots: isOperator && active,
    canExportReports: isAnalyst && active,
    canManageApiKeys: isOwnerOrAdmin && active,
    canManageBilling: isOwnerOrAdmin,
  };
}
