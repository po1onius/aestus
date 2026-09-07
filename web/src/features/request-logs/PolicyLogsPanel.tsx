import { ChevronLeft, ChevronRight, Loader2, RefreshCw, ScrollText } from "lucide-react";
import { useEffect, useState } from "react";
import { requestJson } from "../../api/client";
import { DatePickerInput } from "../../components/DatePickerInput";
import { requestLogPageSize, policyLogsPath } from "../../config";
import { formatDateTimeWithMilliseconds, todayInputValue } from "../../lib/format";
import { cellMainClass, cx, emptyStateClass, iconButton, panelClass, spinnerClass, tableClass } from "../../lib/ui";
import type { ListPolicyLogsResponse, PolicyLogCursor } from "../../types";

interface PolicyLogsPanelProps {
  token: string | null;
  timezone: string;
  refreshRevision: number;
}

export function PolicyLogsPanel({ token, timezone, refreshRevision }: PolicyLogsPanelProps) {
  const [date, setDate] = useState(() => todayInputValue(timezone));
  const [cursors, setCursors] = useState<Array<PolicyLogCursor | null>>([null]);
  const [retryRevision, setRetryRevision] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    data: ListPolicyLogsResponse | null;
    error: string | null;
  } | null>(null);
  const cursor = cursors[cursors.length - 1];
  const params = new URLSearchParams({ date, limit: String(requestLogPageSize) });
  if (cursor) {
    params.set("before_occurred_at", cursor.before_occurred_at);
    params.set("before_id", cursor.before_id);
  }
  const path = `${policyLogsPath}?${params}`;
  const queryKey = JSON.stringify([path, token, refreshRevision, retryRevision]);
  // 条件变化时立即隐藏旧结果；取消请求后也不接受旧响应回写。
  const loading = result?.key !== queryKey;
  const data = loading ? null : result?.data;
  const error = loading ? null : result?.error;

  useEffect(() => {
    const controller = new AbortController();
    void requestJson<ListPolicyLogsResponse>(path, { signal: controller.signal }, token)
      .then((data) => {
        if (!controller.signal.aborted) {
          setResult({ key: queryKey, data, error: null });
        }
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) {
          return;
        }
        console.error("Policy 日志查询失败", { path, error });
        setResult({
          key: queryKey,
          data: null,
          error: error instanceof Error ? error.message : "Policy 日志查询失败",
        });
      });
    return () => controller.abort();
  }, [path, token, queryKey]);

  return (
    <div className={`${panelClass} flex min-h-0 flex-1 flex-col overflow-hidden`}>
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-slate-200 px-4 py-3 dark:border-slate-800">
        <h2 className="text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100">Policy 日志</h2>
        <div className="flex flex-wrap items-center gap-2">
          <DatePickerInput
            value={date}
            max={todayInputValue(timezone)}
            ariaLabel="选择 Policy 日志日期"
            onChange={(value) => {
              if (value) {
                setDate(value);
                setCursors([null]);
              }
            }}
          />
          <div className="flex gap-1.5" aria-label="Policy 日志分页">
            <button
              className={iconButton}
              disabled={loading || cursors.length <= 1}
              onClick={() => setCursors((value) => value.slice(0, -1))}
              title="上一页"
              aria-label="上一页"
            >
              <ChevronLeft size={18} />
            </button>
            <button
              className={iconButton}
              disabled={loading || !data?.next_cursor}
              onClick={() => {
                if (data?.next_cursor) {
                  setCursors((value) => [...value, data.next_cursor]);
                }
              }}
              title="下一页"
              aria-label="下一页"
            >
              <ChevronRight size={18} />
            </button>
          </div>
        </div>
      </div>
      {loading ? (
        <div className={emptyStateClass} role="status">
          <Loader2 className={spinnerClass} size={24} />
          <span>正在加载 Policy 日志</span>
        </div>
      ) : error ? (
        <div className={emptyStateClass} role="alert">
          <span>{error}</span>
          <button className={iconButton} onClick={() => setRetryRevision((value) => value + 1)} title="重新加载" aria-label="重新加载 Policy 日志">
            <RefreshCw size={18} />
          </button>
        </div>
      ) : !data?.items.length ? (
        <div className={emptyStateClass}>
          <ScrollText size={24} />
          <span>当天没有 Policy 日志</span>
        </div>
      ) : (
        <div className="min-h-0 w-full flex-1 overflow-auto overscroll-contain">
          <table className={cx(tableClass, "min-w-[64rem] [&_th]:sticky [&_th]:top-0 [&_th]:z-10")}>
            <colgroup>
              <col className="w-[16rem]" />
              <col className="w-[12rem]" />
              <col className="w-[16rem]" />
              <col className="w-[20rem]" />
            </colgroup>
            <thead>
              <tr>
                <th>时间（{data.timezone}）</th>
                <th>用户名</th>
                <th>账号邮箱</th>
                <th>错误码</th>
              </tr>
            </thead>
            <tbody>
              {data.items.map((log) => (
                <tr key={log.id}>
                  <td><div className={cellMainClass}>{formatDateTimeWithMilliseconds(log.occurred_at, data.timezone)}</div></td>
                  <td><div className={cellMainClass} title={log.username}>{log.username}</div></td>
                  <td><div className={cellMainClass} title={log.account_email ?? undefined}>{log.account_email || "未记录"}</div></td>
                  <td><div className={`${cellMainClass} font-mono`} title={log.error_code}>{log.error_code}</div></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
