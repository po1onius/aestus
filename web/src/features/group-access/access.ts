import { useCallback, useEffect, useMemo, useState } from "react";
import { requestJson } from "../../api/client";
import { providerGroupsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import type {
  DashboardUser,
  GroupPermission,
  UserGroupGrant,
} from "../../types";

export interface ProviderAccess {
  isOwner: boolean;
  canViewAccounts: boolean;
  canViewOfficialApiKeys: boolean;
  canViewProviderResources: boolean;
  has: (groupId: string | null | undefined, permission: GroupPermission) => boolean;
}

/**
 * 将角色和逐组授权收敛成页面可消费的 capability，避免组件继续散落 role 字符串判断。
 * 这只控制界面呈现；所有真实授权仍由后端按资源数据库归属重新校验。
 */
export function createProviderAccess(
  user: DashboardUser | null,
  grants: UserGroupGrant[],
): ProviderAccess {
  const isOwner = user?.role === "tenant_owner";
  const permissionsByGroup = new Map(
    grants.map((grant) => [grant.group_id, new Set<GroupPermission>(grant.permissions)]),
  );
  const canViewAccounts =
    isOwner || grants.some((grant) => grant.permissions.includes("account.view"));
  const canViewOfficialApiKeys =
    isOwner || grants.some((grant) => grant.permissions.includes("official_api_key.view"));
  return {
    isOwner,
    canViewAccounts,
    canViewOfficialApiKeys,
    canViewProviderResources: canViewAccounts || canViewOfficialApiKeys,
    has(groupId, permission) {
      return isOwner || Boolean(groupId && permissionsByGroup.get(groupId)?.has(permission));
    },
  };
}

/** 当前登录人的授权独立加载；授权变化无需重新签发 JWT，也不会膨胀 App 根组件状态。 */
export function useCurrentGroupAccess(
  user: DashboardUser | null,
  token: string | null,
) {
  const [grants, setGrants] = useState<UserGroupGrant[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadedUserId, setLoadedUserId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (!user || !token || user.role !== "tenant_user") {
      setGrants([]);
      setLoading(false);
      setLoadedUserId(user?.role === "tenant_user" ? null : (user?.id ?? null));
      return;
    }
    setLoading(true);
    try {
      setGrants(
        await requestJson<UserGroupGrant[]>(`${providerGroupsPath}/access`, undefined, token),
      );
    } catch (error) {
      setGrants([]);
      showErrorToast("分组授权加载失败", error);
    } finally {
      setLoading(false);
      setLoadedUserId(user.id);
    }
  }, [token, user]);

  useEffect(() => {
    void reload();
  }, [reload]);

  return {
    grants,
    loading,
    ready: user?.role !== "tenant_user" || loadedUserId === user.id,
    reload,
    access: useMemo(() => createProviderAccess(user, grants), [grants, user]),
  };
}
