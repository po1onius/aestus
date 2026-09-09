import { useEffect, useState } from "react";
import { useRequestScope } from "../../api/useRequestScope";
import { usagePath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import { UsagePage } from "../../pages/UsagePage";
import type { UsageResponse } from "../../types";
import { useDashboard, type PageProps } from "../dashboard/context";
const usageLoadErrorToastId = "usage-load-error";

export function UsageScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser, serviceTimezone, theme } = useDashboard();
  const { requestJson, beginRequest } = useRequestScope(authToken);

  const [usage, setUsage] = useState<UsageResponse | null>(null);

  const [usageLoading, setUsageLoading] = useState(false);

  async function loadUsage() {
    const token = authToken;
    const request = beginRequest("loadUsage");
    if (!token || !currentUser) {
      setUsage(null);
      setUsageLoading(false);
      return;
    }

    setUsageLoading(true);
    try {
      const data = await requestJson<UsageResponse>(
        usagePath,
        { signal: request.signal },
        token,
      );
      if (!request.isCurrent()) {
        return;
      }
      setUsage(data);
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("用量数据加载失败", error, usageLoadErrorToastId);
      }
    } finally {
      if (request.isCurrent()) {
        setUsageLoading(false);
      }
    }
  }
  useEffect(() => { void loadUsage(); }, [refreshRevision, serviceTimezone]);
  useEffect(() => { onLoadingChange(usageLoading); return () => onLoadingChange(false); }, [usageLoading, onLoadingChange]);
  return <UsagePage
    theme={theme}
    usage={usage}
    loading={usageLoading}
  />;
}
