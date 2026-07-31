import { ChevronLeft, ChevronRight, Loader2, ScrollText } from "lucide-react";
import { DatePickerInput } from "../components/DatePickerInput";
import { StatusBadge } from "../components/StatusBadge";
import {
  requestLogDetail,
  requestLogEffort,
  requestLogErrorKind,
  requestLogFastMode,
  statusLabel,
  statusTone,
} from "../features/request-logs/utils";
import {
  firstTokenDurationMs,
  formatDateTimeWithMilliseconds,
  formatDuration,
} from "../lib/format";
import {
  cellMainClass,
  cellNoteClass,
  cellWrapClass,
  cx,
  emptyStateClass,
  entryStackClass,
  entryTitleClass,
  iconButton,
  panelClass,
  spinnerClass,
  tableClass,
} from "../lib/ui";
import type { RequestLogCursor, RequestLogRecord } from "../types";

interface RequestLogsPageProps {
  logs: RequestLogRecord[];
  showUsername: boolean;
  loading: boolean;
  date: string;
  nonSuccessOnly: boolean;
  nextCursor: RequestLogCursor | null;
  cursorStack: Array<RequestLogCursor | null>;
  onDateChange: (date: string) => void;
  onNonSuccessOnlyChange: (nonSuccessOnly: boolean) => void;
  onPreviousPage: () => void;
  onNextPage: () => void;
}

export function RequestLogsPage({
  logs,
  showUsername,
  loading,
  date,
  nonSuccessOnly,
  nextCursor,
  cursorStack,
  onDateChange,
  onNonSuccessOnlyChange,
  onPreviousPage,
  onNextPage,
}: RequestLogsPageProps) {
  return (
    <section className="min-h-0 min-w-0 flex-1">
      <div className={`${panelClass} flex min-h-0 flex-col overflow-hidden lg:h-full`}>
        <div className="flex flex-wrap items-start justify-between gap-4 border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <div>
            <h2 className="text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100">请求日志</h2>
          </div>
          <div className="flex flex-wrap items-center justify-end gap-2">
            <div className="flex items-center">
              <DatePickerInput
                value={date}
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
            <table className={`${tableClass} min-w-[90rem] [&_th]:sticky [&_th]:top-0 [&_th]:z-10`}>
              <thead>
                <tr>
                  <th>请求</th>
                  <th>模型</th>
                  <th>请求参数</th>
                  <th>状态</th>
                  <th>时间</th>
                  <th>Token</th>
                  <th>详情</th>
                </tr>
              </thead>
              <tbody>
                {logs.map((log) => (
                  <tr key={log.request_id}>
                    <td>
                      <div className={entryStackClass}>
                        <strong className={entryTitleClass}>{log.route}</strong>
                        <span className={cellNoteClass}>provider: {log.provider}</span>
                        {showUsername && <span className={cellNoteClass}>user: {log.username || "未记录"}</span>}
                        {log.provider_group_name && (
                          <span className={cellNoteClass} title={log.provider_group_id || undefined}>
                            group: {log.provider_group_name}
                          </span>
                        )}
                        <span className={cellNoteClass}>{log.request_id}</span>
                        {log.api_key_name && <span className={cellNoteClass}>key : {log.api_key_name}</span>}
                      </div>
                    </td>
                    <td>
                      <div className={cellMainClass}>{log.model || "未记录"}</div>
                    </td>
                    <td>
                      <div className={cellMainClass}>
                        fast mode: {formatFastMode(requestLogFastMode(log))}
                      </div>
                      <p className={cellNoteClass}>
                        effort: {requestLogEffort(log) || "未记录"}
                      </p>
                      {/* false 表示普通请求、null 表示该协议没有压缩分类，两者都不占用展示空间。 */}
                      {log.is_compaction === true && (
                        <p className="truncate text-xs font-medium leading-5 text-orange-600 dark:text-orange-400">
                          压缩
                        </p>
                      )}
                    </td>
                    <td>
                      <StatusBadge status={statusTone(log)} />
                      <p className={cellNoteClass}>{statusLabel(log)}</p>
                      {requestLogErrorKind(log) && (
                        <p className={cellNoteClass}>kind: {requestLogErrorKind(log)}</p>
                      )}
                    </td>
                    <td>
                      <RequestTimeCell log={log} />
                    </td>
                    <td>
                      <div className={cellMainClass}>{log.total_tokens}</div>
                      <p className={cellNoteClass}>in {log.input_tokens}</p>
                      <p className={cellNoteClass}>out {log.output_tokens}</p>
                      {log.cached_input_tokens > 0 && (
                        <p className={cellNoteClass}>cached {log.cached_input_tokens}</p>
                      )}
                      {log.reasoning_output_tokens > 0 && (
                        <p className={cellNoteClass}>reasoning {log.reasoning_output_tokens}</p>
                      )}
                    </td>
                    <td>
                      <div className={cellWrapClass}>{requestLogDetail(log)}</div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
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
    <>
      <div className={firstTokenClassName}>首字：{formatDuration(firstTokenMs)}</div>
      <p className={cellNoteClass}>总耗时：{formatDuration(log.duration_ms)}</p>
      <p className={cellNoteClass}>
        开始：{formatDateTimeWithMilliseconds(log.request_started_at)}
      </p>
    </>
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
