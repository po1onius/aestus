import { useEffect, useState } from "react";
import { useRequestScope } from "../../api/useRequestScope";
import { requestLogPageSize, requestLogsPath, tenantsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import { shiftDateInputValue, todayInputValue } from "../../lib/format";
import { RequestLogsPage } from "../../pages/RequestLogsPage";
import type { ListRequestLogsResponse, RequestLogCursor, RequestLogRecord, RequestLogView, TenantSummary } from "../../types";
import { useDashboard, type PageProps } from "../dashboard/context";

export function RequestLogsScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser, serviceTimezone, requestLogRetentionDays } = useDashboard();
  const { requestJson, beginRequest } = useRequestScope(authToken);

  const [requestLogs, setRequestLogs] = useState<RequestLogRecord[]>([]);

  const [requestLogView, setRequestLogView] = useState<RequestLogView>("requests");

  const [requestLogDate, setRequestLogDate] = useState(() => todayInputValue(serviceTimezone));

  const [requestLogNonSuccessOnly, setRequestLogNonSuccessOnly] = useState(false);

  const [requestLogTenantId, setRequestLogTenantId] = useState("");

  const [requestLogTenantOptions, setRequestLogTenantOptions] = useState<TenantSummary[]>([]);

  const [requestLogNextCursor, setRequestLogNextCursor] = useState<RequestLogCursor | null>(null);

  const [requestLogCursorStack, setRequestLogCursorStack] = useState<Array<RequestLogCursor | null>>([]);

  const [requestLogCurrentCursor, setRequestLogCurrentCursor] = useState<RequestLogCursor | null>(null);

  const [requestLogsLoading, setRequestLogsLoading] = useState(false);
  const [policyLogsLoading, setPolicyLogsLoading] = useState(false);

  const [requestLogTenantsLoading, setRequestLogTenantsLoading] = useState(false);

  async function loadRequestLogs(
    cursor: RequestLogCursor | null = null,
    cursorStack: Array<RequestLogCursor | null> = requestLogCursorStack,
  ) {
    const token = authToken;
    const request = beginRequest("loadRequestLogs");
    if (!token) {
      resetRequestLogPaging();
      setRequestLogsLoading(false);
      return;
    }

    setRequestLogsLoading(true);
    try {
      const data = await requestJson<ListRequestLogsResponse>(
        `${requestLogsPath}?${requestLogQueryParams(cursor).toString()}`,
        { signal: request.signal },
        token,
      );
      if (!request.isCurrent()) {
        return;
      }
      setRequestLogs(data.items);
      setRequestLogCurrentCursor(cursor);
      setRequestLogCursorStack(cursorStack);
      setRequestLogNextCursor(data.next_cursor);
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("请求日志加载失败", error);
      }
    } finally {
      if (request.isCurrent()) {
        setRequestLogsLoading(false);
      }
    }
  }

  async function loadRequestLogTenantOptions() {
    const token = authToken;
    const request = beginRequest("loadRequestLogTenantOptions");
    if (!token || currentUser?.role !== "platform_admin") {
      setRequestLogTenantOptions([]);
      setRequestLogTenantsLoading(false);
      return;
    }

    setRequestLogTenantsLoading(true);
    try {
      const tenants = await requestJson<TenantSummary[]>(tenantsPath, { signal: request.signal }, token);
      if (request.isCurrent()) {
        setRequestLogTenantOptions(tenants);
      }
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("请求日志租户列表加载失败", error);
      }
    } finally {
      if (request.isCurrent()) {
        setRequestLogTenantsLoading(false);
      }
    }
  }

  function resetRequestLogPaging() {
    setRequestLogs([]);
    setRequestLogNextCursor(null);
    setRequestLogCursorStack([]);
    setRequestLogCurrentCursor(null);
  }

  async function loadNextRequestLogPage() {
    if (!requestLogNextCursor) {
      return;
    }

    const nextStack = [...requestLogCursorStack, requestLogCurrentCursor];
    await loadRequestLogs(requestLogNextCursor, nextStack);
  }

  async function loadPreviousRequestLogPage() {
    if (requestLogCursorStack.length === 0) {
      return;
    }

    const nextStack = requestLogCursorStack.slice(0, -1);
    await loadRequestLogs(requestLogCursorStack[requestLogCursorStack.length - 1] ?? null, nextStack);
  }

  function requestLogQueryParams(cursor: RequestLogCursor | null) {
    const params = new URLSearchParams({
      limit: String(requestLogPageSize),
      date: requestLogDate,
    });

    if (requestLogNonSuccessOnly) {
      params.set("non_success_only", "true");
    }
    if (currentUser?.role === "platform_admin" && requestLogTenantId) {
      params.set("tenant_id", requestLogTenantId);
    }
    if (cursor) {
      params.set("before_started_at", cursor.before_started_at);
      params.set("before_request_id", cursor.before_request_id);
    }

    return params;
  }
  useEffect(() => {
    resetRequestLogPaging();
    if (requestLogView === "requests") void loadRequestLogs(null, []);
    else {
      beginRequest("loadRequestLogs");
      setRequestLogsLoading(false);
    }
  }, [refreshRevision, requestLogDate, requestLogNonSuccessOnly, requestLogTenantId, requestLogView]);
  useEffect(() => { if (currentUser.role === "platform_admin") void loadRequestLogTenantOptions(); }, [refreshRevision]);
  useEffect(() => {
    onLoadingChange(requestLogView === "policy" ? policyLogsLoading : requestLogsLoading);
    return () => onLoadingChange(false);
  }, [requestLogView, policyLogsLoading, requestLogsLoading, onLoadingChange]);
  return <><RequestLogsPage
    showPolicyLogs={currentUser.role === "tenant_owner"}
    view={requestLogView}
    onViewChange={setRequestLogView}
    token={authToken}
    policyRefreshRevision={refreshRevision}
    onPolicyLoadingChange={setPolicyLogsLoading}
    logs={requestLogs}
    showTenant={currentUser.role === "platform_admin"}
    showUsername={currentUser.role !== "tenant_user"}
    tenants={requestLogTenantOptions}
    tenantsLoading={requestLogTenantsLoading}
    selectedTenantId={requestLogTenantId}
    loading={requestLogsLoading}
    date={requestLogDate}
    minDate={shiftDateInputValue(
      todayInputValue(serviceTimezone),
      -(requestLogRetentionDays - 1),
    )}
    maxDate={todayInputValue(serviceTimezone)}
    timezone={serviceTimezone}
    nonSuccessOnly={requestLogNonSuccessOnly}
    nextCursor={requestLogNextCursor}
    cursorStack={requestLogCursorStack}
    onDateChange={setRequestLogDate}
    onTenantChange={setRequestLogTenantId}
    onNonSuccessOnlyChange={setRequestLogNonSuccessOnly}
    onPreviousPage={loadPreviousRequestLogPage}
    onNextPage={loadNextRequestLogPage}
  /></>;
}
