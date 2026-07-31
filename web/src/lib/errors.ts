import { toast } from "sonner";
import {
  ApiRequestError,
  isDashboardAuthError,
  isProviderStateSyncError,
} from "../api/client";

export function errorMessageFrom(error: unknown) {
  if (error instanceof ApiRequestError && error.requestId) {
    return `${error.message}（请求 ID：${error.requestId}）`;
  }
  return error instanceof Error ? error.message : "请求失败，请查看服务日志。";
}

/** 统一错误提示出口，后续错误码和 request ID 映射只需在此维护。 */
export function showErrorToast(title: string, error: unknown, toastId?: string) {
  console.error(`[dashboard] ${title}`, error);
  // 带 token 请求的 401 已由 App 的统一失效处理展示，避免每个业务 catch 再弹一次。
  if (isDashboardAuthError(error)) {
    return;
  }
  if (isProviderStateSyncError(error)) {
    toast.warning("操作已写入数据库，请勿重复提交", {
      description: errorMessageFrom(error),
      duration: Infinity,
      id: toastId,
    });
    return;
  }
  toast.error(title, {
    description: errorMessageFrom(error),
    id: toastId,
  });
}
