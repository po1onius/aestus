import { AlertCircle, Loader2 } from "lucide-react";
import { Modal } from "../../components/Modal";
import { formatDateTime, formatOptionalDateTime, formatPercent, formatTokenCount } from "../../lib/format";
import { spinnerClass } from "../../lib/ui";
import type {
  GptAccount,
  GptAccountQuotaResponse,
  GptQuotaSnapshot,
  GptQuotaWindow,
} from "../../types";
import { creditsLabel, quotaStatusLabel } from "./utils";

interface AccountQuotaDialogProps {
  account: GptAccount;
  response: GptAccountQuotaResponse | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
}

/** GPT 账号额度弹窗集中展示查询状态和上游返回的所有额度窗口。 */
export function AccountQuotaDialog(props: AccountQuotaDialogProps) {
  const accountLabel = props.account.email || props.account.account_id || props.account.id;
  const snapshots = props.response
    ? props.response.snapshots.length > 0
      ? props.response.snapshots
      : props.response.primary
        ? [props.response.primary]
        : []
    : [];

  return (
    <Modal
      titleId="accountQuotaTitle"
      title="账号额度"
      description={`账号：${accountLabel}`}
      className="max-w-4xl"
      closeDisabled={props.loading}
      onClose={props.onClose}
    >
      {props.error ? (
        <div className="flex min-h-36 items-center justify-center gap-3 rounded-lg border border-red-200 bg-red-50 p-5 text-sm text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300">
          <AlertCircle className="shrink-0" size={20} />
          <p className="min-w-0 break-words leading-6">{props.error}</p>
        </div>
      ) : props.loading && !props.response ? (
        <div className="flex min-h-40 items-center justify-center gap-2 rounded-lg border border-slate-200 text-sm text-slate-500 dark:border-slate-800 dark:text-slate-400">
          <Loader2 className={spinnerClass} size={20} />
          正在查询账号额度
        </div>
      ) : props.response ? (
        <div className="grid gap-4">
          {props.response.quota_limit_removed && (
            <div className="rounded-lg border border-emerald-200 bg-emerald-50 px-4 py-3 text-sm text-emerald-800 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-300">
              本次查询确认额度已经恢复，账号的额度限制已解除。
            </div>
          )}

          <dl className="grid gap-3 sm:grid-cols-3">
            <QuotaSummary label="计划" value={props.response.plan_type || "未返回"} />
            <QuotaSummary label="查询时间" value={formatDateTime(props.response.fetched_at)} />
            <QuotaSummary
              label="可用重置次数"
              value={props.response.rate_limit_reset_credits?.available_count.toString() ?? "未返回"}
            />
          </dl>

          {snapshots.length > 0 ? (
            <div className="grid gap-3">
              {snapshots.map((snapshot, index) => (
                <QuotaSnapshotCard key={`${snapshot.limit_id}-${index}`} snapshot={snapshot} />
              ))}
            </div>
          ) : (
            <div className="flex min-h-28 items-center justify-center rounded-lg border border-dashed border-slate-300 px-5 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-400">
              上游没有返回额度窗口。
            </div>
          )}
        </div>
      ) : null}
    </Modal>
  );
}

function QuotaSummary({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-slate-200 bg-slate-50/70 px-4 py-3 dark:border-slate-800 dark:bg-slate-950/40">
      <dt className="text-xs font-medium text-slate-500 dark:text-slate-400">{label}</dt>
      <dd className="mt-1 truncate text-sm font-semibold text-slate-900 dark:text-slate-100" title={value}>
        {value}
      </dd>
    </div>
  );
}

function QuotaSnapshotCard({ snapshot }: { snapshot: GptQuotaSnapshot }) {
  const title = snapshot.limit_name || (snapshot.limit_id === "codex" ? "Codex" : snapshot.limit_id);

  return (
    <section className="rounded-lg border border-slate-200 p-4 dark:border-slate-800">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold text-slate-900 dark:text-slate-100" title={title}>
            {title}
          </h3>
          <p className="mt-1 truncate text-xs text-slate-500 dark:text-slate-400" title={snapshot.limit_id}>
            {snapshot.limit_id}
          </p>
        </div>
        <span className="rounded-full bg-slate-100 px-2.5 py-1 text-xs font-semibold text-slate-700 dark:bg-slate-800 dark:text-slate-300">
          {quotaStatusLabel(snapshot)}
        </span>
      </div>

      {snapshot.primary || snapshot.secondary ? (
        <div className="mt-4 grid gap-3 md:grid-cols-2">
          {snapshot.primary && <QuotaWindowCard label="主窗口" window={snapshot.primary} showGatewayUsage={snapshot.limit_id === "codex"} />}
          {snapshot.secondary && <QuotaWindowCard label="次窗口" window={snapshot.secondary} showGatewayUsage={snapshot.limit_id === "codex"} />}
        </div>
      ) : (
        <p className="mt-4 text-sm text-slate-500 dark:text-slate-400">未返回窗口用量。</p>
      )}

      {snapshot.limit_id === "codex" && (snapshot.primary || snapshot.secondary) && (
        <p className="mt-3 text-xs leading-5 text-slate-500 dark:text-slate-400">
          Token 按请求开始时间统计至本次查询时间，仅包含本网关已记录的用量。
          窗口时间缺失、已过期或超出日志保留期时无法统计。
        </p>
      )}

      {(snapshot.credits || snapshot.individual_limit || snapshot.rate_limit_reached_type) && (
        <dl className="mt-4 grid gap-2 border-t border-slate-100 pt-4 text-xs text-slate-500 dark:border-slate-800 dark:text-slate-400 sm:grid-cols-2">
          {snapshot.credits && (
            <div>
              <dt className="inline font-medium text-slate-700 dark:text-slate-300">Credits：</dt>
              <dd className="inline">{creditsLabel(snapshot.credits)}</dd>
            </div>
          )}
          {snapshot.individual_limit && (
            <div>
              <dt className="inline font-medium text-slate-700 dark:text-slate-300">个人限额：</dt>
              <dd className="inline">
                {formatPercent(snapshot.individual_limit.remaining_percent)} 剩余（{snapshot.individual_limit.remaining} / {snapshot.individual_limit.limit}）
              </dd>
            </div>
          )}
          {snapshot.individual_limit?.resets_at && (
            <div>
              <dt className="inline font-medium text-slate-700 dark:text-slate-300">个人限额重置：</dt>
              <dd className="inline">{formatDateTime(snapshot.individual_limit.resets_at)}</dd>
            </div>
          )}
          {snapshot.rate_limit_reached_type && (
            <div>
              <dt className="inline font-medium text-slate-700 dark:text-slate-300">触发类型：</dt>
              <dd className="inline break-all">{snapshot.rate_limit_reached_type}</dd>
            </div>
          )}
        </dl>
      )}
    </section>
  );
}

function QuotaWindowCard({ label, window, showGatewayUsage }: { label: string; window: GptQuotaWindow; showGatewayUsage: boolean }) {
  return (
    <div className="rounded-lg bg-slate-50 px-4 py-3 dark:bg-slate-950/50">
      <p className="text-xs font-medium text-slate-500 dark:text-slate-400">{label}</p>
      <p className="mt-1 text-lg font-semibold text-slate-900 dark:text-slate-100">
        {formatPercent(window.remaining_percent)} 剩余
      </p>
      <div className="mt-2 grid gap-1 text-xs leading-5 text-slate-500 dark:text-slate-400">
        <span>已用：{formatPercent(window.used_percent)}</span>
        <span>
          窗口：
          {window.window_minutes !== null ? `${window.window_minutes} 分钟` : "未返回"}
        </span>
        <span>重置：{formatOptionalDateTime(window.resets_at)}</span>
        {showGatewayUsage && (
          <>
            <span>窗口开始：{formatOptionalDateTime(window.starts_at)}</span>
            <span className="font-medium text-slate-700 dark:text-slate-300">
              本窗口网关已记录 Token：
              {window.gateway_total_tokens !== null ? formatTokenCount(window.gateway_total_tokens) : "无法统计"}
            </span>
          </>
        )}
      </div>
    </div>
  );
}
