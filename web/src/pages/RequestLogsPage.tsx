import { ChevronLeft, ChevronRight, Loader2, ScrollText } from "lucide-react";
import { AnimatePresence } from "motion/react";
import { useState, type KeyboardEvent } from "react";
import { DatePickerInput } from "../components/DatePickerInput";
import { StatusBadge } from "../components/StatusBadge";
import { RequestLogDetailDialog } from "../features/request-logs/RequestLogDetailDialog";
import { requestLogEffort, requestLogFastMode, statusTone } from "../features/request-logs/utils";
import { firstTokenDurationMs, formatDuration } from "../lib/format";
import {
  cellMainClass,
  cx,
  emptyStateClass,
  entryTitleClass,
  iconButton,
  panelClass,
  spinnerClass,
  tableClass,
  compactInputClass,
} from "../lib/ui";
import type { RequestLogCursor, RequestLogRecord, TenantSummary } from "../types";

interface RequestLogsPageProps {
  logs: RequestLogRecord[];
  showTenant: boolean;
  showUsername: boolean;
  tenants: TenantSummary[];
  tenantsLoading: boolean;
  selectedTenantId: string;
  loading: boolean;
  date: string;
  minDate: string;
  maxDate: string;
  timezone: string;
  nonSuccessOnly: boolean;
  nextCursor: RequestLogCursor | null;
  cursorStack: Array<RequestLogCursor | null>;
  onDateChange: (date: string) => void;
  onTenantChange: (tenantId: string) => void;
  onNonSuccessOnlyChange: (nonSuccessOnly: boolean) => void;
  onPreviousPage: () => void;
  onNextPage: () => void;
}

export function RequestLogsPage({
  logs,
  showTenant,
  showUsername,
  tenants,
  tenantsLoading,
  selectedTenantId,
  loading,
  date,
  minDate,
  maxDate,
  timezone,
  nonSuccessOnly,
  nextCursor,
  cursorStack,
  onDateChange,
  onTenantChange,
  onNonSuccessOnlyChange,
  onPreviousPage,
  onNextPage,
}: RequestLogsPageProps) {
  const [selectedLog, setSelectedLog] = useState<RequestLogRecord | null>(null);

  function handleLogKeyDown(event: KeyboardEvent<HTMLTableRowElement>, log: RequestLogRecord) {
    if (event.key !== "Enter" && event.key !== " ") {
      return;
    }
    event.preventDefault();
    setSelectedLog(log);
  }

  return (
    <section className="min-h-0 min-w-0 flex-1">
      <div className={`${panelClass} flex min-h-0 flex-col overflow-hidden lg:h-full`}>
        <div className="flex flex-wrap items-start justify-between gap-4 border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <div>
            <h2 className="text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100">请求日志</h2>
          </div>
          <div className="flex flex-wrap items-center justify-end gap-2">
            {showTenant && (
              <label>
                <span className="sr-only">按租户查询请求日志</span>
                <select
                  className={`${compactInputClass} min-w-44`}
                  value={selectedTenantId}
                  disabled={loading || tenantsLoading}
                  aria-label="按租户查询请求日志"
                  onChange={(event) => onTenantChange(event.target.value)}
                >
                  <option value="">全部租户</option>
                  {tenants.map((tenant) => (
                    <option key={tenant.id} value={tenant.id}>
                      {tenant.id}{tenant.enabled ? "" : "（已停用）"}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <div className="flex items-center">
              <DatePickerInput
                value={date}
                min={minDate}
                max={maxDate}
                disabled={loading}
                ariaLabel="选择请求日志日期"
                onChange={onDateChange}
              />
            </div>
            <label className="inline-flex min-h-9 cursor-pointer items-center gap-2 rounded-lg border border-slate-300 bg-white px-3 text-xs font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800">
              <input
                className="size-4 accent-indigo-600 dark:accent-indigo-400"
                type="checkbox"
                checked={nonSuccessOnly}
                disabled={loading}
                onChange={(event) => onNonSuccessOnlyChange(event.target.checked)}
              />
              <span>异常/失败</span>
            </label>
            <div className="flex gap-1.5" aria-label="请求日志分页">
              <button
                className={iconButton}
                disabled={loading || cursorStack.length === 0}
                onClick={onPreviousPage}
                title="上一页"
                aria-label="上一页"
              >
                <ChevronLeft size={18} />
              </button>
              <button
                className={iconButton}
                disabled={loading || !nextCursor}
                onClick={onNextPage}
                title="下一页"
                aria-label="下一页"
              >
                <ChevronRight size={18} />
              </button>
            </div>
          </div>
        </div>
        {loading ? (
          <div className={emptyStateClass}>
            <Loader2 className={spinnerClass} size={24} />
            <span>正在加载请求日志</span>
          </div>
        ) : logs.length === 0 ? (
          <div className={emptyStateClass}>
            <ScrollText size={24} />
            <span>还没有请求日志</span>
          </div>
        ) : (
          <div className="min-h-0 w-full flex-1 overflow-auto overscroll-contain">
            <table className={`${tableClass} ${showTenant ? "min-w-[88rem]" : "min-w-[76rem]"} [&_th]:sticky [&_th]:top-0 [&_th]:z-10`}>
              <colgroup>
                {showTenant ? (
                  <>
                    <col className="w-[14%]" />
                    <col className="w-[16%]" />
                    <col className="w-[16%]" />
                    <col className="w-[12%]" />
                    <col className="w-[9%]" />
                    <col className="w-[9%]" />
                    <col className="w-[17%]" />
                    <col className="w-[7%]" />
                  </>
                ) : (
                  <>
                    <col className="w-[20%]" />
                    <col className="w-[18%]" />
                    <col className="w-[13%]" />
                    <col className="w-[10%]" />
                    <col className="w-[10%]" />
                    <col className="w-[18%]" />
                    <col className="w-[11%]" />
                  </>
                )}
              </colgroup>
              <thead>
                <tr>
                  {showTenant && <th>租户 ID</th>}
                  <th>请求</th>
                  <th>模型</th>
                  <th>请求参数</th>
                  <th>强度</th>
                  <th>状态</th>
                  <th>首/总</th>
                  <th>Token</th>
                </tr>
              </thead>
              <tbody>
                {logs.map((log) => (
                  <tr
                    key={log.request_id}
                    className="cursor-pointer transition-colors hover:[&>td]:bg-indigo-50/60 focus-visible:outline-none focus-visible:[&>td]:bg-indigo-50 focus-visible:[&>td]:ring-2 focus-visible:[&>td]:ring-inset focus-visible:[&>td]:ring-indigo-600/30 dark:hover:[&>td]:bg-indigo-950/25 dark:focus-visible:[&>td]:bg-indigo-950/40 dark:focus-visible:[&>td]:ring-indigo-400/35"
                    role="button"
                    tabIndex={0}
                    title="点击查看请求详情"
                    aria-label={`查看请求 ${log.request_id} 的详情`}
                    onClick={() => setSelectedLog(log)}
                    onKeyDown={(event) => handleLogKeyDown(event, log)}
                  >
                    {showTenant && (
                      <td>
                        <div className={cellMainClass} title={log.tenant_id || undefined}>
                          {log.tenant_id || "未归属"}
                        </div>
                      </td>
                    )}
                    <td>
                      <strong className={`${entryTitleClass} block`} title={log.route}>
                        {log.route}
                      </strong>
                    </td>
                    <td>
                      <div className={cellMainClass} title={log.model || undefined}>
                        {log.model || "未记录"}
                      </div>
                    </td>
                    <td>
                      <div className={cellMainClass}>
                        fast mode: {formatFastMode(requestLogFastMode(log))}
                      </div>
                    </td>
                    <td>
                      <div className={cellMainClass}>{requestLogEffort(log) || "未记录"}</div>
                    </td>
                    <td>
                      <StatusBadge status={statusTone(log)} />
                    </td>
                    <td>
                      <RequestTimeCell log={log} />
                    </td>
                    <td>
                      <div className={cellMainClass}>{log.total_tokens}</div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      <AnimatePresence>
        {selectedLog && (
          <RequestLogDetailDialog
            log={selectedLog}
            showUsername={showUsername}
            timezone={timezone}
            onClose={() => setSelectedLog(null)}
          />
        )}
      </AnimatePresence>
    </section>
  );
}

function formatFastMode(fastMode: boolean | null) {
  if (fastMode === null) {
    return "未记录";
  }
  return fastMode ? "开启" : "关闭";
}

function RequestTimeCell({ log }: { log: RequestLogRecord }) {
  const firstTokenMs = firstTokenDurationMs(log);
  const tone = firstTokenDurationTone(firstTokenMs);
  const firstTokenClassName = cx(
    "truncate text-sm font-medium leading-5",
    tone === "fast"
      ? "text-emerald-700 dark:text-emerald-400"
      : tone === "moderate"
        ? "text-amber-700 dark:text-amber-400"
        : tone === "slow"
          ? "text-red-700 dark:text-red-400"
          : "text-slate-500 dark:text-slate-400",
  );

  return (
    <div className="truncate whitespace-nowrap text-sm font-medium leading-5 text-slate-800 dark:text-slate-200">
      <span className={firstTokenClassName}>{formatDuration(firstTokenMs)}</span>
      <span className="text-slate-400 dark:text-slate-500"> / </span>
      <span>{formatDuration(log.duration_ms)}</span>
    </div>
  );
}

/** 5 秒和 10 秒均归入黄色区间，与页面展示规则的边界定义保持一致。 */
function firstTokenDurationTone(durationMs: number | null) {
  if (durationMs === null) {
    return null;
  }
  if (durationMs < 5_000) {
    return "fast";
  }
  if (durationMs <= 10_000) {
    return "moderate";
  }
  return "slow";
}
