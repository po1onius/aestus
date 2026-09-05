import type { DashboardUser } from "../../types";

export function pluginSourceLabel(tenantId: string | null) {
  return tenantId === null ? "平台公共" : "本租户";
}

export function canManagePlugin(user: DashboardUser | null, tenantId: string | null) {
  if (!user) return false;
  return user.role === "platform_admin"
    ? tenantId === null
    : user.role === "tenant_owner" && tenantId !== null && tenantId === user.tenant_id;
}
