import { useCallback, useEffect, useState } from "react";
import { useRequestScope } from "../../api/useRequestScope";
import { tenantsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import type { TenantResourceUsage } from "../../types";

/** 资源与分组总量独立于分页和 Provider；普通用户不查询租户全量数据。 */
export function useTenantResourceUsage(token: string, refreshRevision: number, isOwner: boolean) {
  const { requestJson, beginRequest } = useRequestScope(token);
  const [usage, setUsage] = useState<TenantResourceUsage | null>(null);
  const [loading, setLoading] = useState(isOwner);
  const reload = useCallback(async () => {
    if (!isOwner) return;
    const request = beginRequest("tenant-resource-usage");
    setLoading(true);
    setUsage(null);
    try {
      const result = await requestJson<TenantResourceUsage>(`${tenantsPath}/current/resource-usage`, { signal: request.signal });
      if (request.isCurrent()) setUsage(result);
    } catch (error) {
      if (request.isCurrent()) showErrorToast("租户资源与分组总量加载失败", error);
    } finally {
      if (request.isCurrent()) setLoading(false);
    }
  }, [isOwner, requestJson, beginRequest]);
  useEffect(() => { void reload(); }, [reload, refreshRevision]);
  return { usage, loading, reload };
}
