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
