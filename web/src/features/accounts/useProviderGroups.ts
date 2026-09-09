import { useCallback, useState } from "react";
import { useRequestScope } from "../../api/useRequestScope";
import { providerGroupsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import type { ProviderGroupSummary } from "../../types";

/** 资源页与用户授权页共用查询实现，各自持有与页面同寿命的结果。 */
export function useProviderGroups(token: string) {
  const { requestJson, beginRequest } = useRequestScope(token);
  const [providerGroups, setProviderGroups] = useState<ProviderGroupSummary[]>([]);
  const [providerGroupsLoading, setProviderGroupsLoading] = useState(false);
  const loadProviderGroups = useCallback(async () => {
    const request = beginRequest("provider-groups");
    setProviderGroupsLoading(true);
    try {
      const groups = await requestJson<ProviderGroupSummary[]>(providerGroupsPath, { signal: request.signal }, token);
      if (request.isCurrent()) setProviderGroups(groups);
    } catch (error) {
      if (request.isCurrent()) showErrorToast("Provider 分组加载失败", error);
    } finally {
      if (request.isCurrent()) setProviderGroupsLoading(false);
    }
  }, [token, requestJson, beginRequest]);
  return { providerGroups, providerGroupsLoading, loadProviderGroups };
}
