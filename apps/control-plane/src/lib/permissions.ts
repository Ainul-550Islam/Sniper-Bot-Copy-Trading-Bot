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
 * Operator surface permissions mapped to the backend `require_role` model.
 * In the backend, owner ⊃ operator ⊃ readonly.
 * In the frontend, platform administrators and tenant members with owner/admin/operator
 * roles hold operator permissions. Non-operator tenant roles (analyst, viewer) are denied.
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
 * Evaluates whether an authenticated user and organization role grant operator permissions.
 */
export function getOperatorPermissions(
  user?: { platform_admin?: boolean } | null,
  role?: string | null,
  isSuspended?: boolean,
): OperatorPermissions {
  const isPlatformAdmin = Boolean(user?.platform_admin);
  const normalizedRole = (role ?? "").toLowerCase();
  const isOperatorRole =
    normalizedRole === "owner" ||
    normalizedRole === "admin" ||
    normalizedRole === "operator";

  const allowed = (isPlatformAdmin || isOperatorRole) && !isSuspended;

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
