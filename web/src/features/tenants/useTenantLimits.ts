import { useEffect, useState } from "react";
import { useRequestScope } from "../../api/useRequestScope";
import { authPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import type { MeResponse, TenantLimits } from "../../types";

/** 页面进入及刷新时读取最新策略；后端写事务始终独立执行最终校验。 */
export function useTenantLimits(token: string, refreshRevision: number, isPlatformAdmin = false) {
  const { requestJson, beginRequest } = useRequestScope(token);
  const [limits, setLimits] = useState<TenantLimits | null>(null);
  const [loading, setLoading] = useState(!isPlatformAdmin);
  useEffect(() => {
    if (isPlatformAdmin) return;
    const request = beginRequest("tenant-limits");
    setLoading(true);
    setLimits(null);
    void requestJson<MeResponse>(`${authPath}/me`, { signal: request.signal })
      .then(data => { if (request.isCurrent()) setLimits(data.tenant); })
      .catch(error => { if (request.isCurrent()) showErrorToast("租户限制加载失败", error); })
      .finally(() => { if (request.isCurrent()) setLoading(false); });
  }, [token, refreshRevision, isPlatformAdmin, requestJson, beginRequest]);
  return { limits, loading };
}
