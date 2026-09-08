import { ChevronLeft, ChevronRight, Loader2, RefreshCw, ScrollText } from "lucide-react";
import { useEffect, useState } from "react";
import { requestJson } from "../api/client";
import { DatePickerInput } from "../components/DatePickerInput";
import { auditLogsPath, dashboardListPageSize } from "../config";
import { formatDateTimeWithMilliseconds, formatDuration, todayInputValue } from "../lib/format";
import { cellMainClass, cx, emptyStateClass, iconButton, panelClass, spinnerClass, tableClass } from "../lib/ui";
import type { AuditLogCursor, ListAuditLogsResponse, UserRole } from "../types";

interface AuditLogsPageProps {
  token: string | null;
  timezone: string;
  role: UserRole;
  refreshRevision: number;
  onLoadingChange: (loading: boolean) => void;
}

const roleLabels: Record<string, string> = {
  platform_admin: "平台管理员",
  tenant_owner: "租户 owner",
  tenant_user: "普通用户",
};

export function AuditLogsPage({ token, timezone, role, refreshRevision, onLoadingChange }: AuditLogsPageProps) {
  const [date, setDate] = useState(() => todayInputValue(timezone));
  const [cursors, setCursors] = useState<Array<AuditLogCursor | null>>([null]);
  const [retryRevision, setRetryRevision] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    data: ListAuditLogsResponse | null;
    error: string | null;
  } | null>(null);
  const cursor = cursors[cursors.length - 1];
  const params = new URLSearchParams({ date, limit: String(dashboardListPageSize) });
  if (cursor) {
    params.set("before_occurred_at", cursor.before_occurred_at);
    params.set("before_id", cursor.before_id);
  }
  const path = `${auditLogsPath}?${params}`;
  const queryKey = JSON.stringify([path, token, refreshRevision, retryRevision]);
  const loading = result?.key !== queryKey;
  const data = loading ? null : result?.data;
  const error = loading ? null : result?.error;

  useEffect(() => {
    const controller = new AbortController();
    onLoadingChange(true);
    void requestJson<ListAuditLogsResponse>(path, { signal: controller.signal }, token)
      .then((data) => {
        if (!controller.signal.aborted) setResult({ key: queryKey, data, error: null });
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        console.error("审计日志查询失败", { path, error });
        setResult({ key: queryKey, data: null, error: error instanceof Error ? error.message : "审计日志查询失败" });
      })
      .finally(() => {
        if (!controller.signal.aborted) onLoadingChange(false);
      });
    return () => {
      controller.abort();
      onLoadingChange(false);
    };
  }, [path, token, queryKey, onLoadingChange]);

  return (
    <section className={`${panelClass} flex min-h-0 flex-1 flex-col overflow-hidden`}>
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-slate-200 px-4 py-3 dark:border-slate-800">
        <div>
          <h2 className="text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100">控制台请求审计</h2>
          <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
            {role === "platform_admin" ? "全平台记录，包含未确认身份的请求" : role === "tenant_owner" ? "本租户用户的请求记录" : "你的请求记录"}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <DatePickerInput value={date} max={todayInputValue(timezone)} ariaLabel="选择审计日志日期" onChange={(value) => {
            if (value) { setDate(value); setCursors([null]); }
          }} />
          <div className="flex gap-1.5" aria-label="审计日志分页">
            <button className={iconButton} disabled={loading || cursors.length <= 1} onClick={() => setCursors((value) => value.slice(0, -1))} title="上一页" aria-label="上一页"><ChevronLeft size={18} /></button>
            <button className={iconButton} disabled={loading || !data?.next_cursor} onClick={() => {
              if (data?.next_cursor) setCursors((value) => [...value, data.next_cursor]);
            }} title="下一页" aria-label="下一页"><ChevronRight size={18} /></button>
          </div>
        </div>
      </div>
      {loading ? (
        <div className={emptyStateClass} role="status"><Loader2 className={spinnerClass} size={24} /><span>正在加载审计日志</span></div>
      ) : error ? (
        <div className={emptyStateClass} role="alert">
          <span>{error}</span>
          <button className={iconButton} onClick={() => setRetryRevision((value) => value + 1)} title="重新加载" aria-label="重新加载审计日志"><RefreshCw size={18} /></button>
        </div>
      ) : !data?.items.length ? (
        <div className={emptyStateClass}><ScrollText size={24} /><span>当天没有审计日志</span></div>
      ) : (
        <div className="min-h-0 w-full flex-1 overflow-auto overscroll-contain">
          <table className={cx(tableClass, "min-w-[84rem] [&_th]:sticky [&_th]:top-0 [&_th]:z-10")}>
            <thead><tr><th>时间（{data.timezone}）</th><th>操作者</th><th>租户</th><th>请求</th><th>HTTP 状态 / 耗时</th><th>连接来源 IP</th><th>详情</th></tr></thead>
            <tbody>{data.items.map((log) => (
              <tr key={log.id}>
                <td><div className={cellMainClass}>{formatDateTimeWithMilliseconds(log.occurred_at, data.timezone)}</div></td>
                <td>
                  <div className={cellMainClass}>{log.username ?? "未确认身份"}</div>
                  {log.role && <div className="mt-1 text-xs text-slate-500 dark:text-slate-400">{roleLabels[log.role] ?? log.role}</div>}
                </td>
                <td><div className={cellMainClass}>{log.tenant_id ?? "—"}</div></td>
                <td><div className="max-w-md break-all text-sm text-slate-800 dark:text-slate-200"><strong>{log.method}</strong> {log.path}</div></td>
                <td><div className={cx("whitespace-nowrap text-sm", log.status_code >= 400 ? "text-red-700 dark:text-red-400" : "text-slate-800 dark:text-slate-200")}>{log.status_code} / {formatDuration(log.duration_ms)}</div></td>
                <td><div className={cellMainClass}>{log.peer_ip ?? "未记录"}</div></td>
                <td>
                  <details className="max-w-sm text-xs text-slate-600 dark:text-slate-400">
                    <summary className="cursor-pointer whitespace-nowrap text-sm text-indigo-700 dark:text-indigo-400">查看详情</summary>
                    <dl className="mt-2 space-y-1 break-all">
                      <dt>审计 ID</dt><dd>{log.id}</dd>
                      <dt>请求 ID</dt><dd>{log.request_id ?? "未记录"}</dd>
                      <dt>用户 ID</dt><dd>{log.user_id ?? "未确认身份"}</dd>
                      <dt>User-Agent</dt><dd>{log.user_agent ?? "未记录"}</dd>
                    </dl>
                  </details>
                </td>
              </tr>
            ))}</tbody>
          </table>
        </div>
      )}
    </section>
  );
}
