import type { ReactNode } from "react";
import { Modal } from "../../components/Modal";
import { StatusBadge } from "../../components/StatusBadge";
import {
  firstTokenDurationMs,
  formatDateTimeWithMilliseconds,
  formatDuration,
} from "../../lib/format";
import type { RequestLogRecord } from "../../types";
import {
  requestLogDetail,
  requestLogEffort,
  requestLogErrorKind,
  requestLogFastMode,
  statusLabel,
  statusTone,
} from "./utils";

interface RequestLogDetailDialogProps {
  log: RequestLogRecord;
  showUsername: boolean;
  timezone: string;
  onClose: () => void;
}

export function RequestLogDetailDialog({
  log,
  showUsername,
  timezone,
  onClose,
}: RequestLogDetailDialogProps) {
  return (
    <Modal
      titleId="requestLogDetailTitle"
      title="请求详情"
      description={log.request_id}
      className="max-w-4xl"
      onClose={onClose}
    >
      <div className="grid gap-5">
        <DetailSection title="请求">
          <DetailField label="请求 ID" value={log.request_id} code />
          <DetailField label="上游资源 ID" value={log.resource_id || "未记录"} code />
          <DetailField label="路由" value={log.route} code />
          <DetailField label="Provider" value={log.provider} />
          {showUsername && <DetailField label="用户" value={log.username || "未记录"} />}
          <DetailField label="API Key" value={log.api_key_name || "未记录"} />
          <DetailField label="Provider 分组" value={log.provider_group_name || "未记录"} />
          <DetailField label="分组 ID" value={log.provider_group_id || "未记录"} code />
        </DetailSection>

        <DetailSection title="请求参数">
          <DetailField label="模型" value={log.model || "未记录"} />
          <DetailField label="Fast mode" value={formatFastMode(requestLogFastMode(log))} />
          <DetailField label="强度" value={requestLogEffort(log) || "未记录"} />
          <DetailField label="Service tier" value={log.service_tier || "未记录"} />
          <DetailField label="压缩请求" value={formatCompaction(log.is_compaction)} />
          <DetailField label="Reasoning" value={formatJson(log.reasoning)} code multiline />
        </DetailSection>

        <DetailSection title="状态与耗时">
          <DetailField label="状态" value={<StatusBadge status={statusTone(log)} />} />
          <DetailField label="状态说明" value={statusLabel(log)} />
          <DetailField label="错误类型" value={requestLogErrorKind(log) || "无"} code />
          <DetailField label="首字耗时" value={formatDuration(firstTokenDurationMs(log))} />
          <DetailField label="总耗时" value={formatDuration(log.duration_ms)} />
          <DetailField
            label="请求开始"
            value={formatDateTimeWithMilliseconds(log.request_started_at, timezone)}
          />
          <DetailField
            label="响应开始"
            value={formatOptionalLogDateTime(log.response_started_at, timezone)}
          />
          <DetailField
            label="响应结束"
            value={formatOptionalLogDateTime(log.response_finished_at, timezone)}
          />
        </DetailSection>

        <DetailSection title="Token">
          <DetailField label="总计" value={log.total_tokens} />
          <DetailField label="输入" value={log.input_tokens} />
          <DetailField label="输出" value={log.output_tokens} />
          <DetailField label="缓存输入" value={log.cached_input_tokens} />
          <DetailField label="推理输出" value={log.reasoning_output_tokens} />
        </DetailSection>

        <DetailSection title="详情">
          <pre className="max-w-full whitespace-pre-wrap break-words rounded-lg bg-slate-950 p-4 font-mono text-xs leading-5 text-slate-100 dark:bg-black/40">
            {requestLogDetail(log)}
          </pre>
        </DetailSection>
      </div>
    </Modal>
  );
}

function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="grid gap-2">
      <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">{title}</h3>
      <dl className="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-800">
        {children}
      </dl>
    </section>
  );
}

function DetailField({
  label,
  value,
  code = false,
  multiline = false,
}: {
  label: string;
  value: ReactNode;
  code?: boolean;
  multiline?: boolean;
}) {
  return (
    <div className="grid border-b border-slate-100 last:border-b-0 dark:border-slate-800 sm:grid-cols-[8rem_minmax(0,1fr)]">
      <dt className="bg-slate-50 px-3 py-2.5 text-xs font-medium text-slate-500 dark:bg-slate-950/60 dark:text-slate-400">
        {label}
      </dt>
      <dd
        className={`min-w-0 break-words px-3 py-2.5 text-sm text-slate-800 dark:text-slate-200 ${
          code ? "font-mono text-xs leading-5" : ""
        } ${multiline ? "whitespace-pre-wrap" : ""}`}
      >
        {value}
      </dd>
    </div>
  );
}

function formatOptionalLogDateTime(value: string | null, timezone: string) {
  return value ? formatDateTimeWithMilliseconds(value, timezone) : "未记录";
}

function formatFastMode(fastMode: boolean | null) {
  if (fastMode === null) {
    return "未记录";
  }
  return fastMode ? "开启" : "关闭";
}

function formatCompaction(isCompaction: boolean | null) {
  if (isCompaction === null) {
    return "未分类";
  }
  return isCompaction ? "是" : "否";
}

function formatJson(value: string | null) {
  if (!value) {
    return "未记录";
  }
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}
