import { creditsLabel, quotaPrimarySnapshot, quotaStatusLabel } from "./utils";
import { formatDateTime, formatPercent } from "../../lib/format";
import type { GptAccountQuotaResponse } from "../../types";
import { cellMainClass, cellNoteClass } from "../../lib/ui";

export function QuotaCell({ quota }: { quota?: GptAccountQuotaResponse }) {
  if (!quota) {
    return (
      <>
        <div className={`${cellMainClass} text-slate-500 dark:text-slate-400`}>未查询</div>
        <p className={cellNoteClass}>点击查询额度</p>
      </>
    );
  }

  const snapshot = quotaPrimarySnapshot(quota);
  if (!snapshot) {
    return (
      <>
        <div className={cellMainClass}>未返回额度</div>
        <p className={cellNoteClass}>查询：{formatDateTime(quota.fetched_at)}</p>
      </>
    );
  }

  const window = snapshot.primary ?? snapshot.secondary;
  return (
    <>
      <div className={cellMainClass}>
        {window ? `${formatPercent(window.remaining_percent)} 剩余` : quotaStatusLabel(snapshot)}
      </div>
      <p className={cellNoteClass}>{snapshot.limit_name || snapshot.limit_id}</p>
      {window && (
        <p className={cellNoteClass}>
          已用 {formatPercent(window.used_percent)}
          {window.window_minutes ? ` · ${window.window_minutes} 分钟窗口` : ""}
        </p>
      )}
      {window?.resets_at && <p className={cellNoteClass}>重置：{formatDateTime(window.resets_at)}</p>}
      {snapshot.secondary && (
        <p className={cellNoteClass}>
          次窗口剩余 {formatPercent(snapshot.secondary.remaining_percent)}
        </p>
      )}
      {snapshot.credits && <p className={cellNoteClass}>credits：{creditsLabel(snapshot.credits)}</p>}
      {snapshot.individual_limit && (
        <p className={cellNoteClass}>
          个人限额：{formatPercent(snapshot.individual_limit.remaining_percent)} 剩余
        </p>
      )}
      {snapshot.rate_limit_reached_type && (
        <p className={cellNoteClass}>{snapshot.rate_limit_reached_type}</p>
      )}
      {quota.rate_limit_reset_credits && (
        <p className={cellNoteClass}>重置次数：{quota.rate_limit_reset_credits.available_count}</p>
      )}
      {quota.snapshots.length > 1 && <p className={cellNoteClass}>额度组：{quota.snapshots.length}</p>}
      <p className={cellNoteClass}>查询：{formatDateTime(quota.fetched_at)}</p>
    </>
  );
}
